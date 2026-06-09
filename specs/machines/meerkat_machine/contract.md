# MeerkatMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::meerkat_machine`

## State
- Phase enum: `Initializing | Idle | Attached | Running | Retired | Stopped | Destroyed`
- `session_id`: `Option<SessionId>`
- `active_runtime_id`: `Option<AgentRuntimeId>`
- `active_fence_token`: `Option<FenceToken>`
- `active_runtime_generation`: `Option<Generation>`
- `active_runtime_epoch_id`: `Option<RuntimeEpochId>`
- `current_run_id`: `Option<RunId>`
- `pre_run_phase`: `Option<PreRunPhase>`
- `runtime_stop_deferred`: `Bool`
- `turn_phase`: `TurnPhase`
- `primitive_kind`: `Option<TurnPrimitiveKind>`
- `admitted_content_shape`: `Option<ContentShape>`
- `vision_enabled`: `Bool`
- `image_tool_results_enabled`: `Bool`
- `tool_calls_pending`: `u64`
- `pending_op_refs`: `Set<String>`
- `barrier_operation_ids`: `Set<OperationId>`
- `has_barrier_ops`: `Bool`
- `barrier_satisfied`: `Bool`
- `boundary_count`: `u64`
- `cancel_after_boundary`: `Bool`
- `terminal_outcome`: `Option<TurnTerminalOutcome>`
- `terminal_cause_kind`: `Option<TurnTerminalCauseKind>`
- `last_runtime_apply_failure_cause`: `Option<RuntimeApplyFailureCause>`
- `last_runtime_apply_failure_message`: `Option<String>`
- `runtime_completion_result_run_id`: `Option<RunId>`
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
- `model_routing_switch_approval_reasons`: `Map<String, RoutingSwitchApprovalReason>`
- `model_routing_image_operation_phases`: `Map<String, RoutingImageOperationPhase>`
- `model_routing_image_operation_target_models`: `Map<String, String>`
- `model_routing_image_operation_realtime`: `Map<String, Bool>`
- `model_routing_image_operation_requires_scoped_override`: `Map<String, Bool>`
- `model_routing_image_classified_terminals`: `Map<String, RoutingImageTerminal>`
- `model_routing_image_classified_provider_text`: `Map<String, RoutingProviderTextDisposition>`
- `model_routing_image_terminals`: `Map<String, RoutingImageTerminal>`
- `model_routing_image_terminal_payloads`: `Map<String, String>`
- `model_routing_image_denials`: `Map<String, RoutingDenialReason>`
- `model_routing_image_approval_reasons`: `Map<String, RoutingImageApprovalReason>`
- `model_routing_image_plan_denials`: `Map<String, RoutingImagePlanDenialReason>`
- `model_routing_approval_phases`: `Map<String, RoutingApprovalPhase>`
- `model_routing_approval_parent_kind`: `Map<String, RoutingApprovalParentKind>`
- `registration_phase`: `RegistrationPhase`
- `staged_session_phase`: `StagedSessionPhase`
- `staged_session_id`: `Option<SessionId>`
- `staged_session_keep_alive`: `Option<Bool>`
- `staged_session_llm_identity`: `Option<SessionLlmIdentity>`
- `staged_session_machine_archived_resume_authorized`: `Bool`
- `current_session_llm_identity`: `Option<SessionLlmIdentity>`
- `current_session_capability_surface`: `Option<SessionLlmCapabilitySurface>`
- `current_session_capability_surface_status`: `SessionLlmCapabilitySurfaceStatus`
- `current_session_capability_base_filter`: `ToolFilter`
- `session_llm_reconfigure_previous_capability_surface`: `Option<SessionLlmCapabilitySurface>`
- `session_llm_reconfigure_current_capability_surface`: `Option<SessionLlmCapabilitySurface>`
- `session_llm_reconfigure_capability_changed`: `Bool`
- `session_llm_reconfigure_previous_capability_base_filter`: `ToolFilter`
- `session_llm_reconfigure_current_capability_base_filter`: `ToolFilter`
- `session_llm_reconfigure_committed_visible_set_changed`: `Bool`
- `session_llm_reconfigure_revision_bumped`: `Bool`
- `session_llm_reconfigure_active_visibility_revision`: `u64`
- `mob_operator_authority_present`: `Bool`
- `mob_operator_principal_token`: `Option<OpaquePrincipalToken>`
- `mob_operator_can_create_mobs`: `Bool`
- `mob_operator_can_mutate_profiles`: `Bool`
- `mob_operator_managed_mob_scope`: `Set<String>`
- `mob_operator_spawn_profile_scope`: `Map<String, Set<String>>`
- `mob_operator_caller_provenance`: `Option<MobToolCallerProvenance>`
- `mob_operator_audit_invocation_id`: `Option<String>`
- `drain_phase`: `DrainPhase`
- `drain_mode`: `Option<DrainMode>`
- `next_staged_visibility_revision`: `u64`
- `inherited_base_filter`: `ToolFilter`
- `active_filter`: `ToolFilter`
- `staged_filter`: `ToolFilter`
- `active_visibility_revision`: `u64`
- `staged_visibility_revision`: `u64`
- `active_deferred_names`: `Set<String>`
- `staged_deferred_names`: `Set<String>`
- `requested_visibility_witnesses`: `Map<String, ToolVisibilityWitness>`
- `filter_visibility_witnesses`: `Map<String, ToolVisibilityWitness>`
- `active_deferred_authorities`: `Map<String, ToolVisibilityWitness>`
- `staged_deferred_authorities`: `Map<String, ToolVisibilityWitness>`
- `deferred_visibility_authority_catalog`: `Map<String, ToolVisibilityWitness>`
- `filter_visibility_authority_catalog`: `Map<String, ToolVisibilityWitness>`
- `turn_tool_overlay_allow_active`: `Bool`
- `turn_tool_overlay_allow_names`: `Set<String>`
- `turn_tool_overlay_deny_names`: `Set<String>`
- `input_phases`: `Map<String, InputPhase>`
- `input_terminal_kind`: `Map<String, InputTerminalKind>`
- `input_superseded_by`: `Map<String, String>`
- `input_aggregate_id`: `Map<String, String>`
- `input_abandon_reason`: `Map<String, InputAbandonReason>`
- `input_abandon_attempt_count`: `Map<String, u64>`
- `input_attempt_counts`: `Map<String, u64>`
- `max_stage_attempts`: `u64`
- `input_run_associations`: `Map<String, String>`
- `input_boundary_sequences`: `Map<String, u64>`
- `live_boundary_context_sequence_by_run`: `Map<RunId, u64>`
- `next_admission_seq`: `u64`
- `next_priority_admission_seq`: `u64`
- `input_admission_seq`: `Map<String, u64>`
- `input_lane`: `Map<String, InputLane>`
- `input_recovery_lanes`: `Map<String, InputLane>`
- `admission_authorized_lanes`: `Map<String, InputLane>`
- `admission_authorized_plans`: `Map<String, AdmissionPlanKind>`
- `admission_authorized_existing_actions`: `Map<String, AdmissionExistingQueuedActionKind>`
- `admission_authorized_existing_targets`: `Map<String, String>`
- `admission_idempotency_inputs`: `Map<String, String>`
- `recovered_admitted_inputs`: `Set<String>`
- `recovered_admitted_lanes`: `Map<String, InputLane>`
- `op_statuses`: `Map<String, OperationStatus>`
- `op_completion_seq`: `Map<String, u64>`
- `completion_sequence_claims`: `Set<u64>`
- `completion_feed_sequences`: `Map<String, u64>`
- `completion_feed_kinds`: `Map<String, OperationKind>`
- `completion_feed_terminal_outcomes`: `Map<String, OperationTerminalOutcomeKind>`
- `completion_feed_terminal_payload`: `Map<String, String>`
- `op_terminal_outcomes`: `Map<String, OperationTerminalOutcomeKind>`
- `op_terminal_payload`: `Map<String, String>`
- `op_kinds`: `Map<String, OperationKind>`
- `op_sources`: `Map<String, OperationSource>`
- `op_peer_ready`: `Map<String, Bool>`
- `op_progress_counts`: `Map<String, u64>`
- `active_op_count`: `u64`
- `wait_active`: `Bool`
- `wait_request_id`: `Option<WaitRequestId>`
- `wait_run_id`: `Option<RunId>`
- `wait_operation_ids`: `Set<String>`
- `wait_operation_id_tokens`: `Set<OperationId>`
- `next_completion_seq`: `u64`
- `completion_agent_applied_cursor`: `u64`
- `completion_runtime_observed_cursor`: `u64`
- `completion_runtime_injected_cursor`: `u64`
- `surface_request_phases`: `Map<String, SurfaceRequestPhase>`
- `surface_request_terminal_policies`: `Map<String, SurfaceRequestTerminalPolicy>`
- `live_open_admission_sequence`: `u64`
- `live_active_channel_by_session`: `Map<String, String>`
- `live_channel_session_by_channel`: `Map<String, String>`
- `live_channel_identity_by_channel`: `Map<String, SessionLlmIdentity>`
- `live_refresh_result_sequence`: `u64`
- `live_refresh_queue_acceptance_sequence_by_channel`: `Map<String, u64>`
- `live_refresh_status_by_channel`: `Map<String, LiveRefreshPublicStatus>`
- `live_close_result_sequence`: `u64`
- `live_close_observation_sequence_by_channel`: `Map<String, u64>`
- `live_close_status_by_channel`: `Map<String, LiveClosePublicStatus>`
- `live_command_result_sequence`: `u64`
- `live_command_acceptance_sequence_by_channel`: `Map<String, u64>`
- `live_command_kind_by_channel`: `Map<String, LiveCommandPublicKind>`
- `live_command_rejection_sequence`: `u64`
- `live_command_rejection_reason_by_channel`: `Map<String, LiveCommandRejectionReason>`
- `live_command_rejection_public_error_class_by_channel`: `Map<String, LiveCommandRejectionPublicErrorClass>`
- `live_channel_request_rejection_sequence`: `u64`
- `live_channel_request_rejection_reason_by_channel`: `Map<String, LiveChannelRequestRejectionReason>`
- `live_channel_request_rejection_public_error_class_by_channel`: `Map<String, LiveChannelRequestRejectionPublicErrorClass>`
- `live_webrtc_token_issue_sequence`: `u64`
- `live_webrtc_token_channel_by_token`: `Map<String, String>`
- `live_webrtc_token_expires_at_ms_by_token`: `Map<String, u64>`
- `live_webrtc_consumed_tokens`: `Set<String>`
- `live_webrtc_answer_admission_sequence`: `u64`
- `live_webrtc_answer_result_sequence`: `u64`
- `live_webrtc_answer_observation_sequence_by_channel`: `Map<String, u64>`
- `live_webrtc_answer_status_by_channel`: `Map<String, LiveWebrtcAnswerPublicStatus>`
- `live_websocket_token_issue_sequence`: `u64`
- `live_websocket_token_channel_by_token`: `Map<String, String>`
- `live_websocket_token_expires_at_ms_by_token`: `Map<String, u64>`
- `live_websocket_consumed_tokens`: `Set<String>`
- `live_websocket_token_admission_sequence`: `u64`
- `live_channel_status_result_sequence`: `u64`
- `live_channel_status_observation_sequence_by_channel`: `Map<String, u64>`
- `live_channel_status_by_channel`: `Map<String, LiveChannelPublicStatus>`
- `session_event_stream_open_result_sequence`: `u64`
- `session_event_stream_close_result_sequence`: `u64`
- `session_event_stream_terminal_sequence`: `u64`
- `active_session_event_streams`: `Set<String>`
- `closed_session_event_streams`: `Set<String>`
- `session_event_stream_session_ids`: `Map<String, String>`
- `mob_event_stream_open_result_sequence`: `u64`
- `mob_event_stream_close_result_sequence`: `u64`
- `mob_event_stream_terminal_sequence`: `u64`
- `active_mob_event_streams`: `Set<String>`
- `closed_mob_event_streams`: `Set<String>`
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
- `mcp_server_states`: `Map<McpServerId, McpServerState>`
- `pending_peer_requests`: `Map<PeerCorrelationId, OutboundPeerRequestState>`
- `inbound_peer_requests`: `Map<PeerCorrelationId, InboundPeerRequestState>`
- `inbound_peer_request_lanes`: `Map<PeerCorrelationId, InputLane>`
- `last_session_context_updated_at_ms`: `u64`
- `reserved_interaction_streams`: `Set<PeerCorrelationId>`
- `attached_interaction_streams`: `Set<PeerCorrelationId>`
- `peer_ingress_owner_kind`: `PeerIngressOwnerKind`
- `peer_ingress_comms_runtime_id`: `Option<CommsRuntimeId>`
- `peer_ingress_mob_id`: `Option<MobId>`
- `peer_ingress_authority_phase`: `PeerIngressAuthorityPhaseClass`
- `supervisor_binding_kind`: `SupervisorBindingKind`
- `supervisor_bound_name`: `Option<String>`
- `supervisor_bound_peer_id`: `Option<String>`
- `supervisor_bound_address`: `Option<String>`
- `supervisor_bound_signing_public_key`: `Option<String>`
- `supervisor_bound_epoch`: `Option<u64>`
- `supervisor_publish_pending_name`: `Option<String>`
- `supervisor_publish_pending_peer_id`: `Option<String>`
- `supervisor_publish_pending_address`: `Option<String>`
- `supervisor_publish_pending_signing_public_key`: `Option<String>`
- `supervisor_publish_pending_epoch`: `Option<u64>`
- `supervisor_revoke_pending_name`: `Option<String>`
- `supervisor_revoke_pending_peer_id`: `Option<String>`
- `supervisor_revoke_pending_address`: `Option<String>`
- `supervisor_revoke_pending_signing_public_key`: `Option<String>`
- `supervisor_revoke_pending_epoch`: `Option<u64>`
- `local_endpoint`: `Option<PeerEndpoint>`
- `direct_peer_endpoints`: `Set<PeerEndpoint>`
- `mob_overlay_peer_endpoints`: `Set<PeerEndpoint>`
- `peer_projection_epoch`: `u64`
- `mob_overlay_epoch`: `u64`

## Inputs
- `RegisterSession`(session_id: SessionId)
- `UnregisterSession`(session_id: SessionId, agent_runtime_id: Option<AgentRuntimeId>, fence_token: Option<FenceToken>, generation: Option<Generation>, runtime_epoch_id: Option<RuntimeEpochId>)
- `ReconfigureSessionLlmIdentity`(previous_identity: SessionLlmIdentity, previous_visibility_state: SessionToolVisibilityState, previous_capability_surface: Option<SessionLlmCapabilitySurface>, previous_capability_surface_status: SessionLlmCapabilitySurfaceStatus, previous_capability_base_filter: ToolFilter, view_image_tool_available: Bool, previous_view_image_visible: Bool, next_view_image_visible: Bool, previous_active_visibility_revision: u64, previous_staged_visibility_revision: u64, target_identity: SessionLlmIdentity, target_capability_surface: SessionLlmCapabilitySurface, next_visibility_state: SessionToolVisibilityState, next_capability_base_filter: ToolFilter, next_active_visibility_revision: u64, tool_visibility_delta: SessionToolVisibilityDelta)
- `PrepareBindings`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Option<Generation>, runtime_epoch_id: Option<RuntimeEpochId>, session_id: SessionId)
- `SetPeerIngressContext`(keep_alive: Bool)
- `NotifyDrainExited`(reason: DrainExitReason)
- `CancelAfterBoundary`(reason: String)
- `StagePersistentFilter`(filter: ToolFilter, witnesses: Map<String, ToolVisibilityWitness>)
- `PublishCommittedVisibleSet`(active_filter: ToolFilter, staged_filter: ToolFilter, active_requested_deferred_names: Set<String>, staged_requested_deferred_names: Set<String>, active_deferred_authorities: Map<String, ToolVisibilityWitness>, staged_deferred_authorities: Map<String, ToolVisibilityWitness>, active_visibility_revision: u64, staged_visibility_revision: u64)
- `Recover`
- `Retire`(session_id: SessionId)
- `Reset`
- `StopRuntimeExecutor`(reason: String)
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
- `Ingest`(session_id: SessionId, runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Option<Generation>, runtime_epoch_id: Option<RuntimeEpochId>, work_id: WorkId, origin: WorkOrigin)
- `PublishEvent`(kind: RuntimeEventKind)
- `RuntimeState`(runtime_id: String)
- `AdmitModelRoutingAssistantTurn`
- `BeginImageOperation`(operation_id: String, target_model: String, target_realtime_capable: Bool, requires_approval: Bool, approval_available: Bool, approval_denied: Bool, approval_reason: Option<RoutingImageApprovalReason>, realtime_detach_allowed: Bool, requires_scoped_override: Bool)
- `DenyImageOperationPlan`(operation_id: String, reason: RoutingImagePlanDenialReason, terminal_payload: String)
- `ActivateImageOperationOverride`(operation_id: String, target_model: String, target_realtime_capable: Bool)
- `ClassifyImageOperationTerminal`(operation_id: String, observation: RoutingImageTerminalObservation, http_status_code: Option<u64>, error_code: RoutingImageProviderErrorCode, provider_text: RoutingProviderTextDisposition)
- `CompleteImageOperation`(operation_id: String, terminal: RoutingImageTerminal, terminal_payload: String)
- `RestoreImageOperationOverride`(operation_id: String)
- `LoadBoundaryReceipt`(runtime_id: String, sequence: u64)
- `AcceptWithCompletion`(input_id: InputId, request_immediate_processing: Bool, interrupt_yielding: Bool, wake_if_idle: Bool)
- `AcceptWithoutWake`(input_id: InputId)
- `Recycle`
- `ServiceTurnCommitted`(run_id: RunId)
- `RequestDeferredTools`(authorities: Map<String, ToolVisibilityWitness>)

## Surface-only Inputs
- `ContainsSession`
- `SessionHasExecutor`
- `SessionHasComms`
- `OpsLifecycleRegistry`
- `InputState`
- `ListActiveInputs`
- `RuntimeState`
- `ModelRoutingStatus`
- `LoadBoundaryReceipt`

## Runtime-Internal Inputs
- `ResolveRuntimeOpsLifecycleDurability`(session_id: SessionId, agent_runtime_id: Option<AgentRuntimeId>, fence_token: Option<FenceToken>, generation: Option<Generation>, runtime_epoch_id: Option<RuntimeEpochId>)
- `HydrateSessionLlmState`(current_identity: SessionLlmIdentity, current_capability_surface: Option<SessionLlmCapabilitySurface>, current_capability_surface_status: SessionLlmCapabilitySurfaceStatus, current_capability_base_filter: ToolFilter)
- `ClearSessionLlmState`
- `ResolvePeerIngressReceive`(kind: PeerIngressAdmittedKind, auth_required: Bool, auth_exempt: Bool, trusted: Bool, queued_work_present: Bool, queue_closed: Bool, queue_capacity_available: Bool)
- `ResolvePeerIngressDequeue`(kind: PeerIngressAdmittedKind, auth: PeerIngressAuthClass, queued_work_remaining: Bool)
- `InterruptCurrentRun`
- `ResolveUserInterruptPublicResult`(observation: UserInterruptObservationKind, target_present: Bool, staged_promotion_busy: Bool)
- `StageDeferredSession`(session_id: SessionId, keep_alive: Bool, has_comms_name: Bool, llm_identity: SessionLlmIdentity, machine_archived_resume_authorized: Bool)
- `UpdateDeferredSessionKeepAlive`(session_id: SessionId, keep_alive: Bool, has_comms_name: Bool)
- `UpdateDeferredSessionLlmIdentity`(session_id: SessionId, llm_identity: SessionLlmIdentity)
- `AuthorizeDeferredSessionSystemContextAppend`(session_id: SessionId)
- `BeginDeferredSessionPromotion`(session_id: SessionId)
- `AuthorizeDeferredSessionMachineArchivedResume`(session_id: SessionId)
- `AbandonDeferredSessionPromotion`(session_id: SessionId)
- `FinishDeferredSessionPromotion`(session_id: SessionId)
- `BeginDeferredSessionArchive`(session_id: SessionId)
- `RestoreDeferredSessionArchive`(session_id: SessionId)
- `FinishDeferredSessionArchive`(session_id: SessionId)
- `DropDeferredSession`(session_id: SessionId)
- `ResolveMobOperatorCreateAuthority`(request_kind: MobOperatorAccessRequestKind, principal_token: OpaquePrincipalToken, caller_provenance: Option<MobToolCallerProvenance>, audit_invocation_id: Option<String>)
- `RestoreMobOperatorAuthority`(principal_token: OpaquePrincipalToken, can_create_mobs: Bool, can_mutate_profiles: Bool, managed_mob_scope: Set<String>, spawn_profile_scope: Map<String, Set<String>>, caller_provenance: Option<MobToolCallerProvenance>, audit_invocation_id: Option<String>)
- `SetMobOperatorProfileMutation`(allowed: Bool)
- `SetMobOperatorCreateAuthority`(allowed: Bool)
- `GrantMobOperatorManageMob`(mob_id: String)
- `SetMobOperatorSpawnProfilesInMob`(mob_id: String, profiles: Set<String>)
- `RuntimeExecutorExited`
- `ResolveRuntimeCompletionResult`(run_id: Option<RunId>, terminal: RuntimeCompletionTerminalObservation, finalization: RuntimeCompletionFinalizationObservation)
- `ResolveRuntimeCompletionCleanup`(session_id: SessionId, observation_session_id: SessionId, observation_agent_runtime_id: Option<AgentRuntimeId>, observation_fence_token: Option<FenceToken>, observation_runtime_generation: Option<Generation>, observation_runtime_epoch_id: Option<RuntimeEpochId>, outcome: RuntimeCompletionObservedOutcome, archived_by_authority: Bool, live_session: RuntimeCompletionLiveSessionObservation)
- `ResolveRuntimeCompletionWaitFailure`(session_id: SessionId, failure: RuntimeCompletionWaitFailureObservation)
- `RecoverRuntimeAuthority`(session_id: SessionId, state: RuntimeLifecycleObservedState, agent_runtime_id: Option<AgentRuntimeId>, fence_token: Option<FenceToken>, runtime_generation: Option<Generation>, runtime_epoch_id: Option<RuntimeEpochId>, current_run_id: Option<RunId>, pre_run_phase: Option<PreRunPhase>, silent_intent_overrides: Set<String>)
- `ModelRoutingStatus`(session_id: SessionId)
- `SetModelRoutingBaseline`(baseline_model: String, realtime_capable: Bool)
- `RequestFiniteSwitchTurn`(request_id: String, target_model: String, turns: u64, target_realtime_capable: Bool, requires_approval: Bool, approval_available: Bool, approval_denied: Bool, approval_reason: Option<RoutingSwitchApprovalReason>, realtime_detach_allowed: Bool)
- `RequestUntilChangedSwitchTurn`(request_id: String, target_model: String, target_realtime_capable: Bool, requires_approval: Bool, approval_available: Bool, approval_denied: Bool, approval_reason: Option<RoutingSwitchApprovalReason>, realtime_detach_allowed: Bool)
- `CompleteUntilChangedSwitchTurnReconfigure`(request_id: String)
- `ResolveLiveBoundaryContextReceipt`(run_id: RunId, input_id: String)
- `ResolveAdmissionPlan`(input_id: String, input_kind: AdmissionInputKind, requested_lane: Option<InputLane>, continuation_kind: AdmissionContinuationKind, silent_intent_match: Bool, existing_superseded_input_id: Option<String>, runtime_running: Bool, active_turn_boundary_available: Bool, without_wake: Bool)
- `ResolveAdmissionValidation`(input_id: String, input_kind: AdmissionInputKind, input_origin: AdmissionInputOriginKind, durability: InputDurabilityKind, peer_handling_mode_valid: Bool, peer_response_terminal_structurally_valid: Bool, peer_response_terminal_observed_status: PeerResponseTerminalObservedStatus)
- `ResolveAdmissionIdempotency`(input_id: String, idempotency_key: Option<String>)
- `RegisterAcceptedIdempotency`(input_id: String, idempotency_key: String)
- `NormalizeRecoveredInputLifecycle`(input_id: String, phase: RecoveredInputObservedPhase, applied_boundary_committed: Option<Bool>)
- `ClassifyRecoveredInputDurability`(input_id: String, durability: InputDurabilityKind)
- `ResolveInputPublicLifecycle`(input_id: String, phase: RecoveredInputObservedPhase)
- `ResolveInputPublicTerminalOutcome`(input_id: String, phase: RecoveredInputObservedPhase, terminal_kind: Option<InputTerminalKind>, abandon_reason: Option<InputAbandonReason>)
- `ClassifyInputTerminality`(input_id: String, phase: RecoveredInputObservedPhase, terminal_kind: Option<InputTerminalKind>, abandon_reason: Option<InputAbandonReason>)
- `ClassifyTurnTerminalCauseClass`(cause_kind: Option<TurnTerminalCauseKind>)
- `ClassifyTurnTerminality`
- `ClassifyAssistantOutput`(has_visible_or_actionable: Bool)
- `ClassifyCallTimeout`(source: CallTimeoutSource, timeout_ms: u64)
- `ClassifyLlmFailureRecovery`(failure_kind: Option<LlmRetryFailureKind>, retry_attempt: u64, max_retries: u64)
- `ResolveTurnSurfaceResult`(outcome: TurnTerminalOutcome, cause_class: TerminalCauseClass)
- `AuthorizeStoredInputStateSeed`(input_id: String, phase: RecoveredInputObservedPhase, terminal_kind: Option<InputTerminalKind>, superseded_by: Option<String>, aggregate_id: Option<String>, abandon_reason: Option<InputAbandonReason>, abandon_attempt_count: u64, attempt_count: u64, run_id: Option<String>, boundary_sequence: Option<u64>, admission_sequence: Option<u64>, recovery_lane: Option<InputLane>)
- `ClassifyRuntimeLifecycleState`(state: RuntimeLifecycleObservedState)
- `ClassifyRuntimeLifecycleDurability`(state: RuntimeLifecycleObservedState)
- `ClassifyRuntimeLoopQueueAdmission`(state: RuntimeLifecycleObservedState, current_run_bound: Bool)
- `ResolveVisibleRuntimePhase`(dsl_phase: RuntimeLifecycleObservedState, dsl_pre_run_phase: Option<RuntimeLifecycleObservedState>, control_phase: RuntimeLifecycleObservedState, control_pre_run_phase: Option<RuntimeLifecycleObservedState>, has_runtime_persistence: Bool)
- `Prepare`(session_id: SessionId, run_id: RunId)
- `Commit`(input_id: InputId, run_id: RunId)
- `Fail`(run_id: RunId)
- `CancelRun`(run_id: RunId)
- `RollbackRun`(run_id: RunId)
- `StartConversationRun`(run_id: RunId, primitive_kind: TurnPrimitiveKind, admitted_content_shape: ContentShape, vision_enabled: Bool, image_tool_results_enabled: Bool, max_extraction_retries: u64)
- `StartImmediateAppend`(run_id: RunId)
- `StartImmediateContext`(run_id: RunId)
- `PrimitiveApplied`(run_id: RunId)
- `LlmReturnedToolCalls`(run_id: RunId, tool_count: u64)
- `LlmReturnedTerminal`(run_id: RunId)
- `RegisterPendingOps`(run_id: RunId, op_refs: Set<String>, barrier_operation_ids: Set<OperationId>)
- `ToolCallsResolved`(run_id: RunId)
- `OpsBarrierSatisfied`(run_id: RunId, operation_ids: Set<OperationId>)
- `BoundaryContinue`(run_id: RunId)
- `BoundaryComplete`(run_id: RunId)
- `EnterExtraction`(run_id: RunId, max_extraction_retries: u64)
- `ExtractionStart`(run_id: RunId)
- `ExtractionValidationPassed`(run_id: RunId)
- `ExtractionValidationFailed`(run_id: RunId, error: String)
- `ExtractionFailed`(run_id: RunId, error: String)
- `RecoverableFailure`(run_id: RunId, failure_kind: LlmRetryFailureKind, retry_attempt: u64, max_retries: u64, selected_delay_ms: u64, error: String)
- `FatalFailure`(run_id: RunId, terminal_failure_source: RunFailureSourceKind, error: String)
- `RetryRequested`(run_id: RunId, retry_attempt: u64)
- `CancelNow`(run_id: RunId)
- `RequestCancelAfterBoundary`(run_id: RunId)
- `CancellationObserved`(run_id: RunId)
- `AcknowledgeTerminal`(run_id: RunId, outcome: TurnTerminalOutcome)
- `TurnLimitReached`(run_id: RunId)
- `BudgetExhausted`(run_id: RunId)
- `TimeBudgetExceeded`(run_id: RunId)
- `ForceCancelNoRun`
- `RunCompleted`(run_id: RunId)
- `RunFailed`(run_id: RunId, runtime_apply_failure_cause: Option<RuntimeApplyFailureCause>, runtime_apply_failure_message: Option<String>, machine_terminal_failure_observed: Bool, terminal_failure_source: Option<RunFailureSourceKind>, error: String)
- `RunCancelled`(run_id: RunId)
- `RecoverAdmittedInput`(input_id: String, input_kind: RecoveredInputKind, runtime_boundary: RecoveredRunApplyBoundary, runtime_execution_kind: RecoveredRuntimeExecutionKind, runtime_peer_response_terminal_apply_intent: Option<RecoveredPeerResponseTerminalApplyIntent>, lane: InputLane)
- `RecoverInputLifecycle`(input_id: String, phase: InputPhase, terminal_kind: Option<InputTerminalKind>, superseded_by: Option<String>, aggregate_id: Option<String>, abandon_reason: Option<InputAbandonReason>, abandon_attempt_count: u64, attempt_count: u64, run_id: Option<String>, boundary_sequence: Option<u64>, admission_sequence: Option<u64>, admission_sequence_recovery: Option<RecoveredInputNormalizationReasonKind>, recovery_lane: Option<InputLane>, lane: Option<InputLane>)
- `QueueAccepted`(input_id: String)
- `SteerAccepted`(input_id: String)
- `ChangeLane`(input_id: String, new_lane: InputLane)
- `PrioritizeInput`(input_id: String)
- `DeferInputBehindBacklog`(input_id: String)
- `StageForRun`(input_id: String, run_id: String)
- `IncrementAttemptCount`(input_id: String)
- `RollbackStaged`(input_id: String, lane: InputLane)
- `ResolveStagedRollback`(input_id: String, lane: InputLane)
- `MarkApplied`(input_id: String)
- `MarkAppliedPendingConsumption`(input_id: String)
- `ConsumeInput`(input_id: String)
- `ConsumeOnAccept`(input_id: String)
- `SupersedeInput`(input_id: String, superseded_by: String)
- `CoalesceInput`(input_id: String, aggregate_id: String)
- `AbandonInput`(input_id: String, reason: InputAbandonReason, attempt_count: u64)
- `RecordBoundarySeq`(input_id: String, seq: u64)
- `RegisterOp`(operation_id: String, kind: OperationKind, source: Option<OperationSource>, max_concurrent: Option<u64>)
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
- `ResolveOpLifecycleTransitionRejection`(operation_id: String, action: OpLifecycleActionKind)
- `RecoverOpRecord`(operation_id: String, status: OperationStatus, kind: OperationKind, source: Option<OperationSource>, peer_ready: Bool, progress_count: u64, terminal_outcome: Option<OperationTerminalOutcomeKind>, terminal_payload: Option<String>, completion_sequence: Option<u64>)
- `ClassifyOperationTerminality`(operation_id: String, status: OperationStatus)
- `ClassifyOperationPublicResult`(operation_id: String, status: OperationStatus)
- `ClassifyOperationTransitionIdempotence`(operation_id: String, action: OpLifecycleActionKind, status: OperationStatus)
- `ClassifyOperationCompletionFeed`(operation_id: String, kind: OperationKind)
- `ClassifyOperationCompletionWake`(operation_id: String, kind: OperationKind)
- `ClassifyOperationDurability`(operation_id: String, kind: OperationKind)
- `ClassifyRecoveredOperationRecord`(operation_id: String, status: OperationStatus, kind: OperationKind, terminal_outcome_present: Bool, terminal_payload_present: Bool, completion_sequence_present: Bool)
- `RecoverCompletionFeedEntry`(operation_id: String, kind: OperationKind, terminal_outcome: OperationTerminalOutcomeKind, terminal_payload: String, completion_sequence: u64)
- `RecoverOpsCompletionCursor`(next_completion_seq: u64)
- `RecoverCompletionConsumerCursors`(agent_applied_cursor: u64, runtime_observed_cursor: u64, runtime_injected_cursor: u64)
- `AdvanceAgentCompletionCursor`(cursor: u64)
- `AdvanceRuntimeObservedCompletionCursor`(cursor: u64)
- `AdvanceRuntimeInjectedCompletionCursor`(cursor: u64)
- `EvictCompletedOp`(operation_id: String)
- `CollectCompletedOp`(operation_id: String)
- `ResolveWaitAllAdmission`(wait_request_id: WaitRequestId, operation_id_sequence: Seq<String>, operation_ids: Set<String>, operation_id_tokens: Set<OperationId>, operation_token_by_id: Map<String, OperationId>, operation_id_by_token: Map<OperationId, String>, duplicate_operation_id: Option<String>, not_found_operation_id: Option<String>)
- `RequestWaitAll`(run_id: RunId, wait_request_id: WaitRequestId, operation_id_sequence: Seq<String>, operation_ids: Set<String>, operation_id_tokens: Set<OperationId>, operation_token_by_id: Map<String, OperationId>, operation_id_by_token: Map<OperationId, String>)
- `SatisfyWaitAll`(wait_request_id: WaitRequestId, run_id: RunId, operation_id_tokens: Set<OperationId>)
- `CancelWaitAll`
- `AdmitSurfaceRequest`(request_key: String, terminal_policy: SurfaceRequestTerminalPolicy)
- `ClassifySurfaceRequestTerminal`(request_key: String, success: Bool)
- `CancelSurfaceRequest`(request_key: String)
- `PublishSurfaceRequest`(request_key: String)
- `PublishOrCancelSurfaceRequest`(request_key: String)
- `FinishSurfaceRequestUnpublished`(request_key: String)
- `ResolveLiveOpenAdmission`(session_id: String, channel_id: String, llm_identity: SessionLlmIdentity)
- `AbandonLiveOpenAdmission`(session_id: String, channel_id: String)
- `RecordLiveRefreshQueued`(channel_id: String, queue_acceptance_sequence: u64)
- `RecordLiveCloseClosed`(session_id: String, channel_id: String, close_observation_sequence: u64)
- `RecordLiveCommandAccepted`(channel_id: String, command: LiveCommandPublicKind, command_acceptance_sequence: u64)
- `RecordLiveCommandRejected`(channel_id: String, command: LiveCommandPublicKind, rejection: LiveCommandRejectionReason)
- `RecordLiveChannelRequestRejected`(channel_id: String, request: LiveChannelRequestPublicKind, rejection: LiveChannelRequestRejectionReason)
- `RecordLiveWebrtcTokenIssued`(session_id: String, channel_id: String, token: String, issued_at_ms: u64, ttl_ms: u64)
- `ResolveLiveWebrtcAnswerAdmission`(session_id: String, channel_id: String, token: String, observed_at_ms: u64)
- `RecordLiveWebrtcAnswerAccepted`(session_id: String, channel_id: String, answer_observation_sequence: u64)
- `RecordLiveWebsocketTokenIssued`(session_id: String, channel_id: String, token: String, issued_at_ms: u64, ttl_ms: u64)
- `ResolveLiveWebsocketTokenAdmission`(session_id: String, channel_id: String, token: String, observed_at_ms: u64)
- `RecordSessionEventStreamOpened`(stream_id: String, session_id: String)
- `RecordSessionEventStreamTerminated`(stream_id: String, observation: RpcEventStreamTerminalObservationKind, detail: Option<String>)
- `ResolveSessionEventStreamClose`(stream_id: String)
- `RecordMobEventStreamOpened`(stream_id: String)
- `RecordMobEventStreamTerminated`(stream_id: String, observation: RpcEventStreamTerminalObservationKind, detail: Option<String>)
- `ResolveMobEventStreamClose`(stream_id: String)
- `RecordLiveChannelStatus`(channel_id: String, status: LiveChannelPublicStatus, status_observation_sequence: u64, degradation_reason: Option<LiveChannelDegradationReason>, degradation_detail: Option<String>)
- `SpawnDrain`(mode: DrainMode)
- `StopDrain`
- `StageVisibilityFilter`(filter: ToolFilter, witnesses: Map<String, ToolVisibilityWitness>)
- `ReplaceFilterToolAuthorityCatalog`(catalog: Map<String, ToolVisibilityWitness>)
- `CommitVisibilityFilter`(filter: ToolFilter, revision: u64)
- `StageDeferredNames`(names: Set<String>)
- `ReplaceDeferredToolAuthorityCatalog`(catalog: Map<String, ToolVisibilityWitness>)
- `CommitDeferredNames`(authorities: Map<String, ToolVisibilityWitness>)
- `SetTurnToolOverlay`(allow_active: Bool, allow_names: Set<String>, deny_names: Set<String>)
- `ClearTurnToolOverlay`
- `SyncVisibilityRevisions`(capability_base_filter: ToolFilter, inherited_base_filter: ToolFilter, active_filter: ToolFilter, staged_filter: ToolFilter, active_revision: u64, staged_revision: u64, active_deferred_names: Set<String>, staged_deferred_names: Set<String>, requested_witnesses: Map<String, ToolVisibilityWitness>, filter_witnesses: Map<String, ToolVisibilityWitness>, active_deferred_authorities: Map<String, ToolVisibilityWitness>, staged_deferred_authorities: Map<String, ToolVisibilityWitness>)
- `SurfaceRegister`(surface_id: String)
- `SurfaceSetRemovalTimeout`(timeout_ms: u64)
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
- `McpServerConnectPending`(server_id: McpServerId)
- `McpServerConnected`(server_id: McpServerId)
- `McpServerFailed`(server_id: McpServerId, error: String)
- `McpServerDisconnected`(server_id: McpServerId)
- `McpServerReload`(server_id: McpServerId)
- `PeerRequestSent`(corr_id: PeerCorrelationId)
- `PeerResponseProgressArrived`(corr_id: PeerCorrelationId)
- `PeerResponseTerminalArrived`(corr_id: PeerCorrelationId, disposition: PeerTerminalDisposition)
- `PeerResponseRejected`(corr_id: PeerCorrelationId)
- `PeerRequestTimedOut`(corr_id: PeerCorrelationId)
- `PeerRequestSendFailed`(corr_id: PeerCorrelationId)
- `PeerRequestReceived`(corr_id: PeerCorrelationId, handling_mode: InputLane)
- `PeerResponseReplied`(corr_id: PeerCorrelationId)
- `AdvanceSessionContext`(updated_at_ms: u64)
- `InteractionStreamReserved`(corr_id: PeerCorrelationId)
- `InteractionStreamAttached`(corr_id: PeerCorrelationId)
- `InteractionStreamCompleted`(corr_id: PeerCorrelationId)
- `InteractionStreamExpired`(corr_id: PeerCorrelationId)
- `InteractionStreamClosedEarly`(corr_id: PeerCorrelationId)
- `AttachSessionIngress`(comms_runtime_id: CommsRuntimeId)
- `AttachMobIngress`(comms_runtime_id: CommsRuntimeId, mob_id: MobId)
- `DetachIngress`
- `ResolveSupervisorBindAdmission`(supervisor_peer_id: String, supervisor_epoch: u64, sender_peer_id: Option<String>)
- `ResolveSupervisorBindMaterialAdmission`(address_matches: Bool, sender_matches_supervisor: Bool, expected_peer_id_matches: Bool, bootstrap_token_matches: Bool)
- `ResolveTranscriptEditAdmission`(runtime_running: Bool, has_active_inputs: Bool)
- `ResolveSupervisorAuthorizeAdmission`(supervisor_peer_id: String, supervisor_epoch: u64, sender_peer_id: Option<String>)
- `BindSupervisor`(name: String, peer_id: String, address: String, signing_public_key: String, epoch: u64)
- `AuthorizeSupervisor`(name: String, peer_id: String, address: String, signing_public_key: String, epoch: u64)
- `RequestSupervisorTrustPublish`(name: String, peer_id: String, address: String, signing_public_key: String, epoch: u64)
- `RevokeSupervisor`(peer_id: String, epoch: u64)
- `SupervisorTrustEdgePublished`(peer_id: String, epoch: u64)
- `SupervisorTrustEdgePublishFailed`(peer_id: String, epoch: u64, reason: String)
- `SupervisorTrustEdgeRevoked`(peer_id: String, epoch: u64)
- `SupervisorTrustEdgeRevokeFailed`(peer_id: String, epoch: u64, reason: String)
- `PublishLocalEndpoint`(endpoint: PeerEndpoint)
- `ClearLocalEndpoint`
- `AddDirectPeerEndpoint`(endpoint: PeerEndpoint)
- `RemoveDirectPeerEndpoint`(endpoint: PeerEndpoint)
- `ResolveSupervisorBridgeCommandAdmission`(supervisor_peer_id: String, supervisor_epoch: u64, sender_peer_id: Option<String>)
- `AuthorizeSupervisorMobPeerOverlay`(supervisor_peer_id: String, supervisor_epoch: u64, recipient_peer_id: String, overlay_epoch: u64, endpoints: Set<PeerEndpoint>, endpoint_count: u64, command_peer_id: String, command_endpoint: PeerEndpoint, command_kind: MobPeerOverlayCommandKind)
- `ApplyMobPeerOverlay`(epoch: u64, endpoints: Set<PeerEndpoint>)

## Signals
- `Initialize`
- `BoundaryApplied`(revision: u64)
- `DrainQueuedRun`(run_id: RunId)
- `ClassifyExternalEnvelope`(item_id: String, from_peer: String, envelope_kind: PeerIngressEnvelopeClass, request_intent: String, request_intent_class: PeerIngressRequestClass, lifecycle_kind: PeerIngressLifecycleClass, lifecycle_peer_param: Option<String>, response_status: PeerIngressResponseStatus, in_reply_to: String)
- `ClassifyPeerResponseReply`(status: PeerIngressResponseStatus)
- `ClassifyPlainEvent`(source_name: String)
- `EnsureDrainRunning`

## Effects
- `RuntimeBound`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `RuntimeRetired`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `RuntimeDestroyed`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `TurnRunStarted`(run_id: RunId)
- `TurnBoundaryApplied`(run_id: RunId, boundary_sequence: u64)
- `LiveBoundaryContextReceiptResolved`(run_id: RunId, input_id: String, boundary: AdmissionRunApplyBoundary, boundary_sequence: u64)
- `TurnRunCompleted`(run_id: RunId, outcome: TurnTerminalOutcome)
- `TurnRunFailed`(run_id: RunId, terminal_cause_kind: TurnTerminalCauseKind, error: String)
- `TurnRunCancelled`(run_id: RunId, reason: TurnCancellationReason)
- `TurnCheckCompaction`
- `RequestCancellationAtBoundary`
- `WakeInterrupt`
- `CommittedVisibleSetPublished`(revision: u64)
- `RuntimeNotice`(kind: RuntimeNoticeKind, detail: String)
- `RuntimeEffectFact`(kind: RuntimeEffectKind, reason: String)
- `RuntimeCompletionResultResolved`(session_id: SessionId, agent_runtime_id: Option<AgentRuntimeId>, fence_token: Option<FenceToken>, runtime_generation: Option<Generation>, runtime_epoch_id: Option<RuntimeEpochId>, run_id: Option<RunId>, result_class: RuntimeCompletionResultClass, cleanup_outcome: RuntimeCompletionObservedOutcome)
- `RuntimeCompletionCleanupResolved`(session_id: SessionId, action: RuntimeCompletionCleanupAction, pre_admission_action: RuntimeCompletionPreAdmissionAction)
- `RuntimeCompletionWaitFailureResolved`(session_id: SessionId, failure: RuntimeCompletionWaitFailureObservation, pre_admission_action: RuntimeCompletionPreAdmissionAction, public_error_class: RuntimeCompletionWaitFailurePublicErrorClass, public_reason: RuntimeCompletionWaitFailurePublicReason, resumable: Bool)
- `RuntimeOpsLifecycleDurabilityResolved`(session_id: SessionId, agent_runtime_id: Option<AgentRuntimeId>, fence_token: Option<FenceToken>, generation: Option<Generation>, runtime_epoch_id: Option<RuntimeEpochId>, action: RuntimeOpsLifecycleDurabilityAction)
- `UserInterruptPublicResultResolved`(result: UserInterruptPublicResultKind)
- `ModelRoutingStatusChanged`(topology_epoch: u64)
- `SwitchTurnDenied`(request_id: String, reason: RoutingDenialReason)
- `SwitchTurnPersistentReconfigureRequested`(request_id: String, target_model: String)
- `SwitchTurnFiniteOverrideActivated`(request_id: String, target_model: String, turns_remaining: u64)
- `SwitchTurnFiniteOverrideRestored`(request_id: String)
- `ImageOperationPhaseChanged`(operation_id: String, phase: RoutingImageOperationPhase)
- `ImageOperationTerminalClassified`(operation_id: String, terminal: RoutingImageTerminal, provider_text: RoutingProviderTextDisposition)
- `ImageOperationDenied`(operation_id: String, reason: RoutingDenialReason)
- `ModelRoutingApprovalTerminalized`(approval_id: String, phase: RoutingApprovalPhase)
- `ResolveAdmission`
- `SubmitAdmittedIngressEffect`
- `SubmitRunPrimitive`
- `ResolveCompletionAsTerminated`
- `ApplyControlPlaneCommand`
- `InitiateRecycle`
- `IngressAccepted`
- `AdmissionResolved`(input_id: String, policy_version: u64, policy_apply_mode: AdmissionPolicyApplyMode, policy_wake_mode: AdmissionPolicyWakeMode, policy_queue_mode: AdmissionPolicyQueueMode, policy_consume_point: AdmissionPolicyConsumePoint, policy_drain_policy: AdmissionPolicyDrainPolicy, policy_routing_disposition: AdmissionRoutingDisposition, lane: InputLane, plan: AdmissionPlanKind, queue_action: AdmissionQueueActionKind, existing_action: AdmissionExistingQueuedActionKind, existing_input_id: Option<String>, requires_active_pre_admission: Bool, runtime_boundary: AdmissionRunApplyBoundary, runtime_execution_kind: AdmissionRuntimeExecutionKind, runtime_peer_response_terminal_apply_intent: Option<AdmissionPeerResponseTerminalApplyIntent>, record_transcript: Bool, request_immediate_processing: Bool, interrupt_yielding: Bool, wake_if_idle: Bool, execution_handling_mode: Option<InputLane>, live_interrupt_required: Bool)
- `AdmissionValidationResolved`(input_id: String, result: AdmissionValidationResultKind, reject_reason: Option<AdmissionRejectReasonKind>)
- `AdmissionIdempotencyResolved`(input_id: String, result: AdmissionIdempotencyResultKind, existing_input_id: Option<String>)
- `RecoveredInputLifecycleNormalized`(input_id: String, phase: InputPhase, terminal_kind: Option<InputTerminalKind>, recovered: Bool, abandoned: Bool, requeued: Bool, history_reason: Option<RecoveredInputNormalizationReasonKind>)
- `RecoveredInputDurabilityClassified`(input_id: String, disposition: RecoveredInputRecoveryDisposition)
- `InputPublicLifecycleResolved`(input_id: String, phase: InputPublicLifecycleState)
- `InputPublicTerminalOutcomeResolved`(input_id: String, terminal_outcome: Option<InputPublicTerminalOutcome>)
- `InputBehavioralTerminalityResolved`(input_id: String, terminal: Bool)
- `TurnTerminalCauseClassResolved`(cause_kind: Option<TurnTerminalCauseKind>, cause_class: TerminalCauseClass)
- `TurnTerminalityClassified`(terminal: Bool)
- `AssistantOutputClassified`(empty_response_terminal: Bool)
- `CallTimeoutClassified`(verdict: CallTimeoutVerdict, timeout_ms: u64)
- `LlmFailureRecoveryClassified`(recovery: LlmFailureRecoveryKind)
- `TurnSurfaceResultResolved`(outcome: TurnTerminalOutcome, cause_class: TerminalCauseClass, surface_class: SurfaceResultClass)
- `StoredInputStateSeedAuthorized`(input_id: String)
- `RuntimeLifecycleStateClassified`(state: RuntimeLifecycleObservedState, terminality: RuntimeLifecycleTerminality, input_admission: RuntimeInputAdmission, queue_admission: RuntimeQueueAdmission, prepare_admission: RuntimePrepareAdmission, ingress_admission: RuntimeIngressAdmission)
- `RuntimeLifecycleDurabilityClassified`(state: RuntimeLifecycleObservedState, durable_state: RuntimeLifecycleObservedState)
- `RuntimeLoopQueueAdmissionClassified`(state: RuntimeLifecycleObservedState, current_run_bound: Bool, queue_admission: RuntimeQueueAdmission, run_binding: RuntimeLoopRunBinding)
- `VisibleRuntimePhaseResolved`(publish_control: Bool, selected_raw_phase: RuntimeLifecycleObservedState, visible_phase: RuntimeLifecycleObservedState)
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
- `DiscardRecoveredOperationRecord`(operation_id: String)
- `OperationTerminal`(operation_id: String)
- `OperationNonTerminal`(operation_id: String)
- `OperationPublicResultClassified`(operation_id: String, result: OperationPublicResultClass)
- `OperationCompletionFeedClassified`(operation_id: String, result: OperationCompletionFeedClass)
- `OperationCompletionWakeClassified`(operation_id: String, result: OperationCompletionWakeClass)
- `OperationDurabilityClassified`(operation_id: String, result: OperationDurabilityClass)
- `OperationTransitionIdempotentSuccess`(operation_id: String, action: OpLifecycleActionKind, status: OperationStatus)
- `OperationTransitionNotIdempotent`(operation_id: String, action: OpLifecycleActionKind, status: OperationStatus)
- `EvictCompletedRecord`(operation_id: String)
- `CompletionFeedEntryRecovered`(operation_id: String, seq: u64, kind: OperationKind, terminal_outcome: OperationTerminalOutcomeKind, terminal_payload: String)
- `CompletionProduced`(seq: u64, operation_id: OperationId, kind: OperationKind)
- `AgentCompletionCursorAdvanced`(cursor: u64)
- `RuntimeObservedCompletionCursorAdvanced`(cursor: u64)
- `RuntimeInjectedCompletionCursorAdvanced`(cursor: u64)
- `OpRegistrationAdmissionResolved`(operation_id: String, result: OpRegistrationAdmissionResultKind, reject_reason: Option<OpRegistrationRejectReasonKind>, max_concurrent_limit: Option<u64>, active_op_count: u64)
- `OpLifecycleTransitionRejected`(operation_id: String, action: OpLifecycleActionKind, reason: OpLifecycleRejectReasonKind, status: Option<OperationStatus>)
- `WaitAllAdmissionResolved`(wait_request_id: WaitRequestId, result: WaitAllAdmissionResultKind, reject_reason: Option<WaitAllRejectReasonKind>, rejected_operation_id: Option<String>)
- `WaitAllSatisfied`(wait_request_id: WaitRequestId, run_id: RunId, operation_ids: Set<OperationId>)
- `CollectCompletedResult`
- `SurfaceRequestAdmissionAccepted`(request_key: String)
- `SurfaceRequestAdmissionDuplicate`(request_key: String)
- `SurfaceRequestNotFound`(request_key: String)
- `SurfaceRequestTerminalPublish`(request_key: String)
- `SurfaceRequestTerminalRespondWithoutPublish`(request_key: String)
- `SurfaceRequestCancelled`(request_key: String)
- `SurfaceRequestAlreadyCancelled`(request_key: String)
- `SurfaceRequestAlreadyPublished`(request_key: String)
- `SurfaceRequestAlreadyCompleted`(request_key: String)
- `SurfaceRequestPublished`(request_key: String)
- `SurfaceRequestAlreadyTerminal`(request_key: String, current: SurfaceRequestPhase)
- `SurfaceRequestCancelledBeforePublish`(request_key: String)
- `SurfaceRequestCompleted`(request_key: String)
- `SurfaceRequestSupersededByCancel`(request_key: String)
- `LiveRefreshResultResolved`(channel_id: String, status: LiveRefreshPublicStatus, refresh_enqueued: Bool, sequence: u64, queue_acceptance_sequence: u64)
- `LiveCloseResultResolved`(channel_id: String, status: LiveClosePublicStatus, closed: Bool, sequence: u64, close_observation_sequence: u64)
- `LiveCommandResultResolved`(channel_id: String, command: LiveCommandPublicKind, accepted: Bool, sequence: u64, command_acceptance_sequence: u64)
- `LiveCommandRejectionResolved`(channel_id: String, command: LiveCommandPublicKind, rejection: LiveCommandRejectionReason, public_error_class: LiveCommandRejectionPublicErrorClass, sequence: u64)
- `LiveChannelRequestRejectionResolved`(channel_id: String, request: LiveChannelRequestPublicKind, rejection: LiveChannelRequestRejectionReason, public_error_class: LiveChannelRequestRejectionPublicErrorClass, sequence: u64)
- `LiveWebrtcTokenIssued`(session_id: String, channel_id: String, token: String, expires_at_ms: u64, sequence: u64)
- `LiveWebrtcAnswerAdmissionResolved`(session_id: String, channel_id: String, token: String, admitted: Bool, rejection: Option<LiveWebrtcAnswerAdmissionRejection>, public_error_class: Option<LiveChannelRequestRejectionPublicErrorClass>, sequence: u64)
- `LiveWebrtcAnswerResultResolved`(channel_id: String, status: LiveWebrtcAnswerPublicStatus, answered: Bool, sequence: u64, answer_observation_sequence: u64)
- `LiveWebsocketTokenIssued`(session_id: String, channel_id: String, token: String, expires_at_ms: u64, sequence: u64)
- `LiveWebsocketTokenAdmissionResolved`(session_id: String, channel_id: String, token: String, admitted: Bool, rejection: Option<LiveWebsocketTokenAdmissionRejection>, public_error_class: Option<LiveWebsocketTokenAdmissionPublicErrorClass>, sequence: u64)
- `LiveOpenAdmissionResolved`(session_id: String, channel_id: String, bound_llm_identity: Option<SessionLlmIdentity>, admitted: Bool, rejection: Option<LiveOpenAdmissionRejection>, sequence: u64)
- `LiveOpenAdmissionAbandoned`(session_id: String, channel_id: String, sequence: u64)
- `SessionEventStreamOpenResolved`(stream_id: String, session_id: String, opened: Bool, sequence: u64)
- `SessionEventStreamTerminalResolved`(stream_id: String, session_id: String, reason: RpcEventStreamTerminalReason, error_code: Option<RpcEventStreamTerminalErrorCode>, detail: Option<String>, sequence: u64)
- `SessionEventStreamCloseResolved`(stream_id: String, closed: Bool, already_closed: Bool, sequence: u64)
- `MobEventStreamOpenResolved`(stream_id: String, opened: Bool, sequence: u64)
- `MobEventStreamTerminalResolved`(stream_id: String, reason: RpcEventStreamTerminalReason, error_code: Option<RpcEventStreamTerminalErrorCode>, detail: Option<String>, sequence: u64)
- `MobEventStreamCloseResolved`(stream_id: String, closed: Bool, already_closed: Bool, sequence: u64)
- `LiveChannelStatusResolved`(channel_id: String, status: LiveChannelPublicStatus, sequence: u64, status_observation_sequence: u64, degradation_reason: Option<LiveChannelDegradationReason>, degradation_detail: Option<String>)
- `RealtimeTranscriptAppended`(channel_id: String, item_id: String, text: String, role: RealtimeTranscriptRoleKind, lane: RealtimeTranscriptLaneKind, sequence: u64)
- `EnqueueClassifiedEntry`
- `PeerIngressClassified`(class: PeerIngressInputClass, actionable: Bool, kind: PeerIngressAdmittedKind, auth: PeerIngressAuthClass, lifecycle_kind: Option<PeerIngressLifecycleClass>, lifecycle_peer: Option<String>, request_id: Option<String>, response_terminality: Option<PeerIngressResponseTerminality>)
- `PeerResponseReplyClassified`(response_terminality: PeerIngressResponseTerminality)
- `PeerIngressReceiveResolved`(outcome: PeerIngressReceiveOutcomeClass, admission_diagnostic: Option<PeerIngressAdmissionDiagnosticClass>, phase: PeerIngressAuthorityPhaseClass)
- `PeerIngressDequeueResolved`(phase: PeerIngressAuthorityPhaseClass)
- `SpawnDrainTask`
- `ScheduleSurfaceCompletion`(surface_id: String, operation: ExternalToolSurfaceDeltaOperation, pending_task_sequence: u64, staged_intent_sequence: u64, applied_at_turn: u64)
- `RefreshVisibleSurfaceSet`(snapshot_epoch: u64)
- `EmitExternalToolDelta`(surface_id: String, operation: ExternalToolSurfaceDeltaOperation, phase: ExternalToolSurfaceDeltaPhase, cause: Option<ExternalToolSurfaceFailureCause>)
- `CloseSurfaceConnection`(surface_id: String)
- `RejectSurfaceCall`(surface_id: String, cause: ExternalToolSurfaceFailureCause)
- `PublishSupervisorTrustEdge`(local_endpoint: Option<PeerEndpoint>, peer_id: String, name: String, address: String, signing_public_key: Option<String>, epoch: u64)
- `RevokeSupervisorTrustEdge`(local_endpoint: Option<PeerEndpoint>, peer_id: String, epoch: u64)
- `McpServerStateChanged`(server_id: McpServerId, new_state: McpServerState)
- `McpServerReloadRequested`(server_id: McpServerId)
- `PeerInteractionStateChanged`(corr_id: PeerCorrelationId, new_state: OutboundPeerRequestState)
- `PeerInteractionCleanup`(corr_id: PeerCorrelationId)
- `InboundPeerInteractionStateChanged`(corr_id: PeerCorrelationId, new_state: InboundPeerRequestState)
- `SessionContextAdvanced`(updated_at_ms: u64)
- `InteractionStreamStateChanged`(corr_id: PeerCorrelationId, new_state: InteractionStreamState)
- `InteractionStreamCleanup`(corr_id: PeerCorrelationId)
- `LocalEndpointChanged`(endpoint: Option<PeerEndpoint>)
- `SupervisorBindAdmissionResolved`(result: SupervisorBindAdmissionResultKind, rejection: Option<SupervisorBindRejectionKind>)
- `SupervisorBindMaterialAdmissionResolved`(verdict: SupervisorBindMaterialAdmissionKind)
- `TranscriptEditAdmissionResolved`(verdict: TranscriptEditAdmissionKind)
- `SupervisorAuthorizeAdmissionResolved`(result: SupervisorAuthorizeAdmissionResultKind, rejection: Option<SupervisorAuthorizeRejectionKind>, previous_name: Option<String>, previous_peer_id: Option<String>, previous_address: Option<String>, previous_signing_public_key: Option<String>, previous_epoch: Option<u64>)
- `SupervisorBridgeCommandAdmissionResolved`(result: SupervisorBridgeCommandAdmissionResultKind, rejection: Option<SupervisorBridgeCommandRejectionKind>)
- `SessionLlmReconfigurePlanResolved`(previous_capability_surface: Option<SessionLlmCapabilitySurface>, current_capability_surface: Option<SessionLlmCapabilitySurface>, capability_changed: Bool, previous_capability_base_filter: ToolFilter, current_capability_base_filter: ToolFilter, committed_visible_set_changed: Bool, revision_bumped: Bool, active_visibility_revision: u64)
- `PeerProjectionChanged`(peer_projection_epoch: u64)
- `CommsTrustReconcileRequested`(local_endpoint: Option<PeerEndpoint>, peer_projection_epoch: u64, direct_peer_endpoints: Set<PeerEndpoint>, mob_overlay_peer_endpoints: Set<PeerEndpoint>)

## Helpers
- `deferred_authority_has_identity`(witness: ToolVisibilityWitness) -> `Bool`
- `op_lifecycle_action_status_valid`(action: OpLifecycleActionKind, status: OperationStatus) -> `Bool`
- `operation_status_terminal`(status: OperationStatus) -> `Bool`
- `llm_failure_kind_recoverable`(kind: LlmRetryFailureKind) -> `Bool`
- `operation_source_valid`(source: Option<OperationSource>) -> `Bool`
- `op_lifecycle_transition_rejection_idempotent`(action: OpLifecycleActionKind, status: OperationStatus) -> `Bool`
- `wait_operation_token_witness_valid`(operation_ids: Set<String>, operation_id_tokens: Set<OperationId>, operation_token_by_id: Map<String, OperationId>, operation_id_by_token: Map<OperationId, String>) -> `Bool`
- `deferred_authorities_have_identity`(names: Set<String>, witnesses: Map<String, ToolVisibilityWitness>) -> `Bool`
- `meerkat_tool_visibility_filter_names`(filter: ToolFilter) -> `Set<String>`
- `meerkat_tool_visibility_filter_has_identity_witnesses`(filter: ToolFilter, witnesses: Map<String, ToolVisibilityWitness>) -> `Bool`
- `meerkat_tool_visibility_authorities_match_names`(names: Set<String>, witnesses: Map<String, ToolVisibilityWitness>, authorities: Map<String, ToolVisibilityWitness>) -> `Bool`
- `meerkat_tool_visibility_authorities_are_catalog_backed`(authorities: Map<String, ToolVisibilityWitness>, authority_catalog: Map<String, ToolVisibilityWitness>) -> `Bool`
- `meerkat_tool_visibility_filter_has_catalog_witnesses`(filter: ToolFilter, witnesses: Map<String, ToolVisibilityWitness>, authority_catalog: Map<String, ToolVisibilityWitness>) -> `Bool`
- `meerkat_tool_visibility_filter_witnesses_are_catalog_backed`(witnesses: Map<String, ToolVisibilityWitness>, authority_catalog: Map<String, ToolVisibilityWitness>) -> `Bool`
- `meerkat_tool_visibility_names_are_catalog_backed`(names: Set<String>, authority_catalog: Map<String, ToolVisibilityWitness>) -> `Bool`
- `meerkat_tool_visibility_publish_matches_catalog`(active_filter: ToolFilter, staged_filter: ToolFilter, active_deferred_authorities: Map<String, ToolVisibilityWitness>, staged_deferred_authorities: Map<String, ToolVisibilityWitness>, filter_witnesses: Map<String, ToolVisibilityWitness>, deferred_authority_catalog: Map<String, ToolVisibilityWitness>, filter_authority_catalog: Map<String, ToolVisibilityWitness>) -> `Bool`
- `meerkat_tool_visibility_state_replacement_matches`(inherited_base_filter: ToolFilter, active_filter: ToolFilter, staged_filter: ToolFilter, active_requested_deferred_names: Set<String>, staged_requested_deferred_names: Set<String>, requested_witnesses: Map<String, ToolVisibilityWitness>, filter_witnesses: Map<String, ToolVisibilityWitness>, active_deferred_authorities: Map<String, ToolVisibilityWitness>, staged_deferred_authorities: Map<String, ToolVisibilityWitness>, deferred_authority_catalog: Map<String, ToolVisibilityWitness>, filter_authority_catalog: Map<String, ToolVisibilityWitness>, active_visibility_revision: u64, staged_visibility_revision: u64) -> `Bool`
- `meerkat_session_llm_filter_has_view_image_only`(filter: ToolFilter) -> `Bool`
- `meerkat_session_llm_capability_base_filter_matches`(target_capability_surface: SessionLlmCapabilitySurface, next_capability_base_filter: ToolFilter) -> `Bool`
- `meerkat_session_llm_hydrated_capability_base_filter_matches`(current_capability_surface: Option<SessionLlmCapabilitySurface>, current_capability_surface_status: SessionLlmCapabilitySurfaceStatus, current_capability_base_filter: ToolFilter) -> `Bool`
- `meerkat_session_llm_capability_base_filter_replacement_matches`(current_capability_surface: Option<SessionLlmCapabilitySurface>, current_capability_surface_status: SessionLlmCapabilitySurfaceStatus, current_capability_base_filter: ToolFilter, next_capability_base_filter: ToolFilter) -> `Bool`
- `meerkat_session_llm_filter_allows_view_image`(filter: ToolFilter) -> `Bool`
- `meerkat_session_llm_visibility_state_allows_view_image`(view_image_tool_available: Bool, visibility_state: SessionToolVisibilityState) -> `Bool`
- `meerkat_session_llm_expected_next_visibility_revision`(committed_visible_set_changed: Bool, previous_active_visibility_revision: u64, previous_staged_visibility_revision: u64) -> `u64`
- `meerkat_session_llm_visibility_shape_matches`(previous_visibility_state: SessionToolVisibilityState, next_visibility_state: SessionToolVisibilityState, previous_capability_base_filter: ToolFilter, next_capability_base_filter: ToolFilter, previous_active_visibility_revision: u64, previous_staged_visibility_revision: u64, next_active_visibility_revision: u64) -> `Bool`
- `meerkat_session_llm_visibility_delta_matches`(tool_visibility_delta: SessionToolVisibilityDelta, previous_capability_base_filter: ToolFilter, next_capability_base_filter: ToolFilter, committed_visible_set_changed: Bool) -> `Bool`
- `meerkat_session_llm_visibility_reconfigure_plan_matches`(previous_visibility_state: SessionToolVisibilityState, next_visibility_state: SessionToolVisibilityState, previous_capability_base_filter: ToolFilter, next_capability_base_filter: ToolFilter, view_image_tool_available: Bool, previous_view_image_visible: Bool, next_view_image_visible: Bool, previous_active_visibility_revision: u64, previous_staged_visibility_revision: u64, next_active_visibility_revision: u64, tool_visibility_delta: SessionToolVisibilityDelta) -> `Bool`

## Invariants
- `fence_requires_bound_runtime`
- `runtime_generation_requires_bound_runtime`
- `runtime_epoch_requires_bound_runtime`
- `runtime_binding_identity_is_typed`
- `running_has_current_run`
- `deferred_stop_requires_active_runtime_phase`
- `current_run_only_while_running_or_retired`
- `current_run_has_pre_run_phase`
- `staged_surface_ops_are_known_and_sequenced`
- `staged_reload_surfaces_are_active`
- `peer_ingress_owner_consistency`
- `supervisor_binding_consistency`
- `supervisor_revoke_pending_consistency`
- `supervisor_publish_pending_consistency`

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

### `StageDeferredSession`
- From: `Initializing`
- On: `StageDeferredSession`(session_id, keep_alive, has_comms_name, llm_identity, machine_archived_resume_authorized)
- Guards:
  - `not_already_staged`
  - `no_staged_session_id`
  - `keep_alive_requires_comms_name`
- To: `Initializing`

### `UpdateDeferredSessionKeepAlive`
- From: `Initializing`
- On: `UpdateDeferredSessionKeepAlive`(session_id, keep_alive, has_comms_name)
- Guards:
  - `staged_or_promoting`
  - `session_matches`
  - `keep_alive_requires_comms_name`
- To: `Initializing`

### `BeginDeferredSessionPromotion`
- From: `Initializing`
- On: `BeginDeferredSessionPromotion`(session_id)
- Guards:
  - `staged`
  - `session_matches`
- To: `Initializing`

### `UpdateDeferredSessionLlmIdentity`
- From: `Initializing`
- On: `UpdateDeferredSessionLlmIdentity`(session_id, llm_identity)
- Guards:
  - `staged_or_promoting`
  - `session_matches`
- To: `Initializing`

### `AuthorizeDeferredSessionSystemContextAppendStaged`
- From: `Initializing`
- On: `AuthorizeDeferredSessionSystemContextAppend`(session_id)
- Guards:
  - `staged`
  - `session_matches`
- To: `Initializing`

### `AuthorizeDeferredSessionSystemContextAppendPromoting`
- From: `Initializing`
- On: `AuthorizeDeferredSessionSystemContextAppend`(session_id)
- Guards:
  - `promoting`
  - `session_matches`
- To: `Initializing`

### `AuthorizeDeferredSessionMachineArchivedResume`
- From: `Initializing`
- On: `AuthorizeDeferredSessionMachineArchivedResume`(session_id)
- Guards:
  - `promoting`
  - `session_matches`
- To: `Initializing`

### `AbandonDeferredSessionPromotion`
- From: `Initializing`
- On: `AbandonDeferredSessionPromotion`(session_id)
- Guards:
  - `promoting`
  - `session_matches`
- To: `Initializing`

### `FinishDeferredSessionPromotion`
- From: `Initializing`
- On: `FinishDeferredSessionPromotion`(session_id)
- Guards:
  - `promoting`
  - `session_matches`
- To: `Initializing`

### `BeginDeferredSessionArchive`
- From: `Initializing`
- On: `BeginDeferredSessionArchive`(session_id)
- Guards:
  - `staged`
  - `session_matches`
- To: `Initializing`

### `RestoreDeferredSessionArchive`
- From: `Initializing`
- On: `RestoreDeferredSessionArchive`(session_id)
- Guards:
  - `closing`
  - `session_matches`
- To: `Initializing`

### `FinishDeferredSessionArchive`
- From: `Initializing`
- On: `FinishDeferredSessionArchive`(session_id)
- Guards:
  - `closing`
  - `session_matches`
- To: `Initializing`

### `DropDeferredSessionStaged`
- From: `Initializing`
- On: `DropDeferredSession`(session_id)
- Guards:
  - `staged`
  - `session_matches`
- To: `Initializing`

### `DropDeferredSessionPromoting`
- From: `Initializing`
- On: `DropDeferredSession`(session_id)
- Guards:
  - `promoting`
  - `session_matches`
- To: `Initializing`

### `DropDeferredSessionClosing`
- From: `Initializing`
- On: `DropDeferredSession`(session_id)
- Guards:
  - `closing`
  - `session_matches`
- To: `Initializing`

### `ResolveMobOperatorCreateAuthority`
- From: `Initializing`
- On: `ResolveMobOperatorCreateAuthority`(request_kind, principal_token, caller_provenance, audit_invocation_id)
- Guards:
  - `explicit_enable`
- To: `Initializing`

### `ResolveMobOperatorCreateAuthorityAbsent`
- From: `Initializing`
- On: `ResolveMobOperatorCreateAuthority`(request_kind, principal_token, caller_provenance, audit_invocation_id)
- Guards:
  - `not_explicit_enable`
- To: `Initializing`

### `RestoreMobOperatorAuthority`
- From: `Initializing`
- On: `RestoreMobOperatorAuthority`(principal_token, can_create_mobs, can_mutate_profiles, managed_mob_scope, spawn_profile_scope, caller_provenance, audit_invocation_id)
- To: `Initializing`

### `SetMobOperatorProfileMutation`
- From: `Initializing`
- On: `SetMobOperatorProfileMutation`(allowed)
- Guards:
  - `authority_present`
- To: `Initializing`

### `SetMobOperatorCreateAuthority`
- From: `Initializing`
- On: `SetMobOperatorCreateAuthority`(allowed)
- Guards:
  - `authority_present`
- To: `Initializing`

### `GrantMobOperatorManageMob`
- From: `Initializing`
- On: `GrantMobOperatorManageMob`(mob_id)
- Guards:
  - `authority_present`
- To: `Initializing`

### `SetMobOperatorSpawnProfilesInMob`
- From: `Initializing`
- On: `SetMobOperatorSpawnProfilesInMob`(mob_id, profiles)
- Guards:
  - `authority_present`
- To: `Initializing`

### `UnregisterSessionIdle`
- From: `Idle`
- On: `UnregisterSession`(session_id, agent_runtime_id, fence_token, generation, runtime_epoch_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_matches_observation`
  - `fence_binding_matches_observation`
  - `generation_binding_matches_observation`
  - `epoch_binding_matches_observation`
- To: `Idle`

### `UnregisterSessionAttached`
- From: `Attached`
- On: `UnregisterSession`(session_id, agent_runtime_id, fence_token, generation, runtime_epoch_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_matches_observation`
  - `fence_binding_matches_observation`
  - `generation_binding_matches_observation`
  - `epoch_binding_matches_observation`
- To: `Idle`

### `UnregisterSessionRunning`
- From: `Running`
- On: `UnregisterSession`(session_id, agent_runtime_id, fence_token, generation, runtime_epoch_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_matches_observation`
  - `fence_binding_matches_observation`
  - `generation_binding_matches_observation`
  - `epoch_binding_matches_observation`
- To: `Idle`

### `UnregisterSessionRetired`
- From: `Retired`
- On: `UnregisterSession`(session_id, agent_runtime_id, fence_token, generation, runtime_epoch_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_matches_observation`
  - `fence_binding_matches_observation`
  - `generation_binding_matches_observation`
  - `epoch_binding_matches_observation`
- To: `Idle`

### `UnregisterSessionStopped`
- From: `Stopped`
- On: `UnregisterSession`(session_id, agent_runtime_id, fence_token, generation, runtime_epoch_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_matches_observation`
  - `fence_binding_matches_observation`
  - `generation_binding_matches_observation`
  - `epoch_binding_matches_observation`
- To: `Idle`

### `ResolveRuntimeOpsLifecycleDurabilityIdle`
- From: `Idle`
- On: `ResolveRuntimeOpsLifecycleDurability`(session_id, agent_runtime_id, fence_token, generation, runtime_epoch_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_matches_observation`
  - `fence_binding_matches_observation`
  - `generation_binding_matches_observation`
  - `epoch_binding_matches_observation`
- Emits: `RuntimeOpsLifecycleDurabilityResolved`
- To: `Idle`

### `ResolveRuntimeOpsLifecycleDurabilityAttached`
- From: `Attached`
- On: `ResolveRuntimeOpsLifecycleDurability`(session_id, agent_runtime_id, fence_token, generation, runtime_epoch_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_matches_observation`
  - `fence_binding_matches_observation`
  - `generation_binding_matches_observation`
  - `epoch_binding_matches_observation`
- Emits: `RuntimeOpsLifecycleDurabilityResolved`
- To: `Attached`

### `ResolveRuntimeOpsLifecycleDurabilityRunning`
- From: `Running`
- On: `ResolveRuntimeOpsLifecycleDurability`(session_id, agent_runtime_id, fence_token, generation, runtime_epoch_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_matches_observation`
  - `fence_binding_matches_observation`
  - `generation_binding_matches_observation`
  - `epoch_binding_matches_observation`
- Emits: `RuntimeOpsLifecycleDurabilityResolved`
- To: `Running`

### `ResolveRuntimeOpsLifecycleDurabilityRetired`
- From: `Retired`
- On: `ResolveRuntimeOpsLifecycleDurability`(session_id, agent_runtime_id, fence_token, generation, runtime_epoch_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_matches_observation`
  - `fence_binding_matches_observation`
  - `generation_binding_matches_observation`
  - `epoch_binding_matches_observation`
- Emits: `RuntimeOpsLifecycleDurabilityResolved`
- To: `Retired`

### `ResolveRuntimeOpsLifecycleDurabilityStopped`
- From: `Stopped`
- On: `ResolveRuntimeOpsLifecycleDurability`(session_id, agent_runtime_id, fence_token, generation, runtime_epoch_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_matches_observation`
  - `fence_binding_matches_observation`
  - `generation_binding_matches_observation`
  - `epoch_binding_matches_observation`
- Emits: `RuntimeOpsLifecycleDurabilityResolved`
- To: `Stopped`

### `HydrateSessionLlmStateIdle`
- From: `Idle`
- On: `HydrateSessionLlmState`(current_identity, current_capability_surface, current_capability_surface_status, current_capability_base_filter)
- Guards:
  - `session_registered`
  - `capability_base_filter_matches_surface`
- To: `Idle`

### `HydrateSessionLlmStateAttached`
- From: `Attached`
- On: `HydrateSessionLlmState`(current_identity, current_capability_surface, current_capability_surface_status, current_capability_base_filter)
- Guards:
  - `session_registered`
  - `runtime_is_bound`
  - `capability_base_filter_matches_surface`
- To: `Attached`

### `HydrateSessionLlmStateRunning`
- From: `Running`
- On: `HydrateSessionLlmState`(current_identity, current_capability_surface, current_capability_surface_status, current_capability_base_filter)
- Guards:
  - `session_registered`
  - `runtime_is_bound`
  - `capability_base_filter_matches_surface`
- To: `Running`

### `ReconfigureSessionLlmIdentityAttached`
- From: `Attached`
- On: `ReconfigureSessionLlmIdentity`(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, previous_capability_base_filter, view_image_tool_available, previous_view_image_visible, next_view_image_visible, previous_active_visibility_revision, previous_staged_visibility_revision, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta)
- Guards:
  - `session_registered`
  - `runtime_is_bound`
  - `previous_identity_matches_current`
  - `previous_capability_surface_matches_current`
  - `previous_capability_surface_status_matches_current`
  - `previous_capability_base_filter_matches_current`
  - `next_capability_base_filter_matches_target`
  - `visibility_reconfigure_plan_matches`
- Emits: `SessionLlmReconfigurePlanResolved`
- To: `Attached`

### `ReconfigureSessionLlmIdentityRunning`
- From: `Running`
- On: `ReconfigureSessionLlmIdentity`(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, previous_capability_base_filter, view_image_tool_available, previous_view_image_visible, next_view_image_visible, previous_active_visibility_revision, previous_staged_visibility_revision, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta)
- Guards:
  - `session_registered`
  - `runtime_is_bound`
  - `previous_identity_matches_current`
  - `previous_capability_surface_matches_current`
  - `previous_capability_surface_status_matches_current`
  - `previous_capability_base_filter_matches_current`
  - `next_capability_base_filter_matches_target`
  - `visibility_reconfigure_plan_matches`
- Emits: `SessionLlmReconfigurePlanResolved`
- To: `Running`

### `ReconfigureSessionLlmIdentityIdle`
- From: `Idle`
- On: `ReconfigureSessionLlmIdentity`(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, previous_capability_base_filter, view_image_tool_available, previous_view_image_visible, next_view_image_visible, previous_active_visibility_revision, previous_staged_visibility_revision, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta)
- Guards:
  - `session_registered`
  - `previous_identity_matches_current`
  - `previous_capability_surface_matches_current`
  - `previous_capability_surface_status_matches_current`
  - `previous_capability_base_filter_matches_current`
  - `next_capability_base_filter_matches_target`
  - `visibility_reconfigure_plan_matches`
- Emits: `SessionLlmReconfigurePlanResolved`
- To: `Idle`

### `ClearSessionLlmStateIdle`
- From: `Idle`
- On: `ClearSessionLlmState`()
- Guards:
  - `session_registered`
- To: `Idle`

### `ClearSessionLlmStateAttached`
- From: `Attached`
- On: `ClearSessionLlmState`()
- Guards:
  - `session_registered`
- To: `Attached`

### `ClearSessionLlmStateRunning`
- From: `Running`
- On: `ClearSessionLlmState`()
- Guards:
  - `session_registered`
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
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `approval_unavailable`
- Emits: `SwitchTurnDenied`
- To: `Idle`

### `RequestFiniteSwitchTurnApprovalUnavailableAttached`
- From: `Attached`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `approval_unavailable`
- Emits: `SwitchTurnDenied`
- To: `Attached`

### `RequestFiniteSwitchTurnApprovalUnavailableRunning`
- From: `Running`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `approval_unavailable`
- Emits: `SwitchTurnDenied`
- To: `Running`

### `RequestFiniteSwitchTurnApprovalDeniedIdle`
- From: `Idle`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `approval_denied`
  - `approval_reason_present`
- Emits: `SwitchTurnDenied`, `ModelRoutingApprovalTerminalized`
- To: `Idle`

### `RequestFiniteSwitchTurnApprovalDeniedAttached`
- From: `Attached`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `approval_denied`
  - `approval_reason_present`
- Emits: `SwitchTurnDenied`, `ModelRoutingApprovalTerminalized`
- To: `Attached`

### `RequestFiniteSwitchTurnApprovalDeniedRunning`
- From: `Running`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `approval_denied`
  - `approval_reason_present`
- Emits: `SwitchTurnDenied`, `ModelRoutingApprovalTerminalized`
- To: `Running`

### `RequestFiniteSwitchTurnScopedConflictIdle`
- From: `Idle`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `scoped_conflict`
- Emits: `SwitchTurnDenied`
- To: `Idle`

### `RequestFiniteSwitchTurnScopedConflictAttached`
- From: `Attached`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `scoped_conflict`
- Emits: `SwitchTurnDenied`
- To: `Attached`

### `RequestFiniteSwitchTurnScopedConflictRunning`
- From: `Running`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `scoped_conflict`
- Emits: `SwitchTurnDenied`
- To: `Running`

### `RequestFiniteSwitchTurnAcceptedIdle`
- From: `Idle`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `positive_turns`
  - `approval_satisfied`
  - `no_scoped_conflict`
- To: `Idle`

### `RequestFiniteSwitchTurnAcceptedAttached`
- From: `Attached`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `positive_turns`
  - `approval_satisfied`
  - `no_scoped_conflict`
- To: `Attached`

### `RequestFiniteSwitchTurnAcceptedRunning`
- From: `Running`
- On: `RequestFiniteSwitchTurn`(request_id, target_model, turns, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `positive_turns`
  - `approval_satisfied`
  - `no_scoped_conflict`
- To: `Running`

### `RequestUntilChangedSwitchTurnAcceptedIdle`
- From: `Idle`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `approval_satisfied`
- Emits: `SwitchTurnPersistentReconfigureRequested`
- To: `Idle`

### `RequestUntilChangedSwitchTurnAcceptedAttached`
- From: `Attached`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `approval_satisfied`
- Emits: `SwitchTurnPersistentReconfigureRequested`
- To: `Attached`

### `RequestUntilChangedSwitchTurnAcceptedRunning`
- From: `Running`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `approval_satisfied`
- Emits: `SwitchTurnPersistentReconfigureRequested`
- To: `Running`

### `RequestUntilChangedSwitchTurnApprovalUnavailableIdle`
- From: `Idle`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `approval_unavailable`
- Emits: `SwitchTurnDenied`
- To: `Idle`

### `RequestUntilChangedSwitchTurnApprovalUnavailableAttached`
- From: `Attached`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `approval_unavailable`
- Emits: `SwitchTurnDenied`
- To: `Attached`

### `RequestUntilChangedSwitchTurnApprovalUnavailableRunning`
- From: `Running`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `baseline_known`
  - `approval_unavailable`
- Emits: `SwitchTurnDenied`
- To: `Running`

### `RequestUntilChangedSwitchTurnApprovalDeniedIdle`
- From: `Idle`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `approval_denied`
  - `approval_reason_present`
- Emits: `SwitchTurnDenied`, `ModelRoutingApprovalTerminalized`
- To: `Idle`

### `RequestUntilChangedSwitchTurnApprovalDeniedAttached`
- From: `Attached`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `approval_denied`
  - `approval_reason_present`
- Emits: `SwitchTurnDenied`, `ModelRoutingApprovalTerminalized`
- To: `Attached`

### `RequestUntilChangedSwitchTurnApprovalDeniedRunning`
- From: `Running`
- On: `RequestUntilChangedSwitchTurn`(request_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed)
- Guards:
  - `approval_denied`
  - `approval_reason_present`
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
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `operation_in_operation_conflict`
- Emits: `ImageOperationDenied`
- To: `Idle`

### `BeginImageOperationScopedConflictAttached`
- From: `Attached`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `operation_in_operation_conflict`
- Emits: `ImageOperationDenied`
- To: `Attached`

### `BeginImageOperationScopedConflictRunning`
- From: `Running`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `operation_in_operation_conflict`
- Emits: `ImageOperationDenied`
- To: `Running`

### `BeginImageOperationApprovalUnavailableIdle`
- From: `Idle`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `approval_unavailable`
- Emits: `ImageOperationDenied`
- To: `Idle`

### `BeginImageOperationApprovalUnavailableAttached`
- From: `Attached`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `approval_unavailable`
- Emits: `ImageOperationDenied`
- To: `Attached`

### `BeginImageOperationApprovalUnavailableRunning`
- From: `Running`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `approval_unavailable`
- Emits: `ImageOperationDenied`
- To: `Running`

### `BeginImageOperationApprovalDeniedIdle`
- From: `Idle`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `approval_denied`
  - `approval_reason_present`
- Emits: `ImageOperationDenied`, `ModelRoutingApprovalTerminalized`
- To: `Idle`

### `BeginImageOperationApprovalDeniedAttached`
- From: `Attached`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `approval_denied`
  - `approval_reason_present`
- Emits: `ImageOperationDenied`, `ModelRoutingApprovalTerminalized`
- To: `Attached`

### `BeginImageOperationApprovalDeniedRunning`
- From: `Running`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `approval_denied`
  - `approval_reason_present`
- Emits: `ImageOperationDenied`, `ModelRoutingApprovalTerminalized`
- To: `Running`

### `BeginImageOperationAcceptedIdle`
- From: `Idle`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `baseline_known`
  - `no_operation_in_operation`
  - `approval_satisfied`
- Emits: `ImageOperationPhaseChanged`
- To: `Idle`

### `BeginImageOperationAcceptedAttached`
- From: `Attached`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `baseline_known`
  - `no_operation_in_operation`
  - `approval_satisfied`
- Emits: `ImageOperationPhaseChanged`
- To: `Attached`

### `BeginImageOperationAcceptedRunning`
- From: `Running`
- On: `BeginImageOperation`(operation_id, target_model, target_realtime_capable, requires_approval, approval_available, approval_denied, approval_reason, realtime_detach_allowed, requires_scoped_override)
- Guards:
  - `baseline_known`
  - `no_operation_in_operation`
  - `approval_satisfied`
- Emits: `ImageOperationPhaseChanged`
- To: `Running`

### `DenyImageOperationPlanIdle`
- From: `Idle`
- On: `DenyImageOperationPlan`(operation_id, reason, terminal_payload)
- Guards:
  - `operation_not_recorded`
- Emits: `ImageOperationPhaseChanged`
- To: `Idle`

### `DenyImageOperationPlanAttached`
- From: `Attached`
- On: `DenyImageOperationPlan`(operation_id, reason, terminal_payload)
- Guards:
  - `operation_not_recorded`
- Emits: `ImageOperationPhaseChanged`
- To: `Attached`

### `DenyImageOperationPlanRunning`
- From: `Running`
- On: `DenyImageOperationPlan`(operation_id, reason, terminal_payload)
- Guards:
  - `operation_not_recorded`
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

### `ClassifyImageOperationTerminalGeneratedIdle`
- From: `Idle`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `generated_observation`
- Emits: `ImageOperationTerminalClassified`
- To: `Idle`

### `ClassifyImageOperationTerminalGeneratedAttached`
- From: `Attached`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `generated_observation`
- Emits: `ImageOperationTerminalClassified`
- To: `Attached`

### `ClassifyImageOperationTerminalGeneratedRunning`
- From: `Running`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `generated_observation`
- Emits: `ImageOperationTerminalClassified`
- To: `Running`

### `ClassifyImageOperationTerminalEmptyIdle`
- From: `Idle`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `empty_observation`
- Emits: `ImageOperationTerminalClassified`
- To: `Idle`

### `ClassifyImageOperationTerminalEmptyAttached`
- From: `Attached`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `empty_observation`
- Emits: `ImageOperationTerminalClassified`
- To: `Attached`

### `ClassifyImageOperationTerminalEmptyRunning`
- From: `Running`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `empty_observation`
- Emits: `ImageOperationTerminalClassified`
- To: `Running`

### `ClassifyImageOperationTerminalMechanicalFailureIdle`
- From: `Idle`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `mechanical_failure`
- Emits: `ImageOperationTerminalClassified`
- To: `Idle`

### `ClassifyImageOperationTerminalMechanicalFailureAttached`
- From: `Attached`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `mechanical_failure`
- Emits: `ImageOperationTerminalClassified`
- To: `Attached`

### `ClassifyImageOperationTerminalMechanicalFailureRunning`
- From: `Running`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `mechanical_failure`
- Emits: `ImageOperationTerminalClassified`
- To: `Running`

### `ClassifyImageOperationTerminalTimeoutIdle`
- From: `Idle`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `provider_error_observation`
  - `timeout_evidence`
- Emits: `ImageOperationTerminalClassified`
- To: `Idle`

### `ClassifyImageOperationTerminalTimeoutAttached`
- From: `Attached`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `provider_error_observation`
  - `timeout_evidence`
- Emits: `ImageOperationTerminalClassified`
- To: `Attached`

### `ClassifyImageOperationTerminalTimeoutRunning`
- From: `Running`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `provider_error_observation`
  - `timeout_evidence`
- Emits: `ImageOperationTerminalClassified`
- To: `Running`

### `ClassifyImageOperationTerminalCancelledIdle`
- From: `Idle`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `provider_http_error`
  - `cancelled_status`
- Emits: `ImageOperationTerminalClassified`
- To: `Idle`

### `ClassifyImageOperationTerminalCancelledAttached`
- From: `Attached`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `provider_http_error`
  - `cancelled_status`
- Emits: `ImageOperationTerminalClassified`
- To: `Attached`

### `ClassifyImageOperationTerminalCancelledRunning`
- From: `Running`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `provider_http_error`
  - `cancelled_status`
- Emits: `ImageOperationTerminalClassified`
- To: `Running`

### `ClassifyImageOperationTerminalSafetyFilteredIdle`
- From: `Idle`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `provider_error_observation`
  - `safety_code`
- Emits: `ImageOperationTerminalClassified`
- To: `Idle`

### `ClassifyImageOperationTerminalSafetyFilteredAttached`
- From: `Attached`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `provider_error_observation`
  - `safety_code`
- Emits: `ImageOperationTerminalClassified`
- To: `Attached`

### `ClassifyImageOperationTerminalSafetyFilteredRunning`
- From: `Running`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `provider_error_observation`
  - `safety_code`
- Emits: `ImageOperationTerminalClassified`
- To: `Running`

### `ClassifyImageOperationTerminalRefusedIdle`
- From: `Idle`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `provider_error_observation`
  - `refusal_code`
- Emits: `ImageOperationTerminalClassified`
- To: `Idle`

### `ClassifyImageOperationTerminalRefusedAttached`
- From: `Attached`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `provider_error_observation`
  - `refusal_code`
- Emits: `ImageOperationTerminalClassified`
- To: `Attached`

### `ClassifyImageOperationTerminalRefusedRunning`
- From: `Running`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `provider_error_observation`
  - `refusal_code`
- Emits: `ImageOperationTerminalClassified`
- To: `Running`

### `ClassifyImageOperationTerminalProviderFailedIdle`
- From: `Idle`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `provider_error_observation`
  - `not_timeout`
  - `not_cancelled`
  - `not_safety`
  - `not_refusal`
- Emits: `ImageOperationTerminalClassified`
- To: `Idle`

### `ClassifyImageOperationTerminalProviderFailedAttached`
- From: `Attached`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `provider_error_observation`
  - `not_timeout`
  - `not_cancelled`
  - `not_safety`
  - `not_refusal`
- Emits: `ImageOperationTerminalClassified`
- To: `Attached`

### `ClassifyImageOperationTerminalProviderFailedRunning`
- From: `Running`
- On: `ClassifyImageOperationTerminal`(operation_id, observation, http_status_code, error_code, provider_text)
- Guards:
  - `operation_recorded`
  - `provider_error_observation`
  - `not_timeout`
  - `not_cancelled`
  - `not_safety`
  - `not_refusal`
- Emits: `ImageOperationTerminalClassified`
- To: `Running`

### `CompleteImageOperationIdle`
- From: `Idle`
- On: `CompleteImageOperation`(operation_id, terminal, terminal_payload)
- Guards:
  - `operation_active`
  - `operation_requires_scoped_override`
  - `terminal_classified`
- Emits: `ImageOperationPhaseChanged`
- To: `Idle`

### `CompleteImageOperationAttached`
- From: `Attached`
- On: `CompleteImageOperation`(operation_id, terminal, terminal_payload)
- Guards:
  - `operation_active`
  - `operation_requires_scoped_override`
  - `terminal_classified`
- Emits: `ImageOperationPhaseChanged`
- To: `Attached`

### `CompleteImageOperationRunning`
- From: `Running`
- On: `CompleteImageOperation`(operation_id, terminal, terminal_payload)
- Guards:
  - `operation_active`
  - `operation_requires_scoped_override`
  - `terminal_classified`
- Emits: `ImageOperationPhaseChanged`
- To: `Running`

### `CompleteImageOperationWithoutScopedOverrideIdle`
- From: `Idle`
- On: `CompleteImageOperation`(operation_id, terminal, terminal_payload)
- Guards:
  - `operation_plan_resolved`
  - `operation_does_not_require_scoped_override`
  - `no_operation_override_active`
  - `terminal_classified`
- Emits: `ImageOperationPhaseChanged`
- To: `Idle`

### `CompleteImageOperationWithoutScopedOverrideAttached`
- From: `Attached`
- On: `CompleteImageOperation`(operation_id, terminal, terminal_payload)
- Guards:
  - `operation_plan_resolved`
  - `operation_does_not_require_scoped_override`
  - `no_operation_override_active`
  - `terminal_classified`
- Emits: `ImageOperationPhaseChanged`
- To: `Attached`

### `CompleteImageOperationWithoutScopedOverrideRunning`
- From: `Running`
- On: `CompleteImageOperation`(operation_id, terminal, terminal_payload)
- Guards:
  - `operation_plan_resolved`
  - `operation_does_not_require_scoped_override`
  - `no_operation_override_active`
  - `terminal_classified`
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
  - `deferred_authorities_match_machine_catalog`
  - `existing_requested_witness_preserved`
  - `existing_staged_authority_preserved`
- To: `Idle`

### `RequestDeferredToolsAttached`
- From: `Attached`
- On: `RequestDeferredTools`(authorities)
- Guards:
  - `session_registered`
  - `deferred_authorities_non_empty`
  - `deferred_authorities_have_identity`
  - `deferred_authorities_match_machine_catalog`
  - `existing_requested_witness_preserved`
  - `existing_staged_authority_preserved`
- To: `Attached`

### `RequestDeferredToolsRunning`
- From: `Running`
- On: `RequestDeferredTools`(authorities)
- Guards:
  - `session_registered`
  - `deferred_authorities_non_empty`
  - `deferred_authorities_have_identity`
  - `deferred_authorities_match_machine_catalog`
  - `existing_requested_witness_preserved`
  - `existing_staged_authority_preserved`
- To: `Running`

### `RequestDeferredToolsRetired`
- From: `Retired`
- On: `RequestDeferredTools`(authorities)
- Guards:
  - `session_registered`
  - `deferred_authorities_non_empty`
  - `deferred_authorities_have_identity`
  - `deferred_authorities_match_machine_catalog`
  - `existing_requested_witness_preserved`
  - `existing_staged_authority_preserved`
- To: `Retired`

### `RequestDeferredToolsStopped`
- From: `Stopped`
- On: `RequestDeferredTools`(authorities)
- Guards:
  - `session_registered`
  - `deferred_authorities_non_empty`
  - `deferred_authorities_have_identity`
  - `deferred_authorities_match_machine_catalog`
  - `existing_requested_witness_preserved`
  - `existing_staged_authority_preserved`
- To: `Stopped`

### `PrepareBindingsIdempotentInitializing`
- From: `Initializing`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation, runtime_epoch_id, session_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_exact`
  - `fence_binding_exact`
  - `generation_binding_exact`
  - `epoch_binding_exact`
  - `typed_binding_identity_present`
- To: `Initializing`

### `PrepareBindingsIdempotentIdle`
- From: `Idle`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation, runtime_epoch_id, session_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_exact`
  - `fence_binding_exact`
  - `generation_binding_exact`
  - `epoch_binding_exact`
  - `typed_binding_identity_present`
- To: `Idle`

### `PrepareBindingsIdempotentAttached`
- From: `Attached`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation, runtime_epoch_id, session_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_exact`
  - `fence_binding_exact`
  - `generation_binding_exact`
  - `epoch_binding_exact`
  - `typed_binding_identity_present`
- To: `Attached`

### `PrepareBindingsIdempotentRunning`
- From: `Running`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation, runtime_epoch_id, session_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_exact`
  - `fence_binding_exact`
  - `generation_binding_exact`
  - `epoch_binding_exact`
  - `typed_binding_identity_present`
- To: `Running`

### `PrepareBindingsIdempotentRetired`
- From: `Retired`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation, runtime_epoch_id, session_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_exact`
  - `fence_binding_exact`
  - `generation_binding_exact`
  - `epoch_binding_exact`
  - `typed_binding_identity_present`
- To: `Retired`

### `PrepareBindingsIdempotentStopped`
- From: `Stopped`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation, runtime_epoch_id, session_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_exact`
  - `fence_binding_exact`
  - `generation_binding_exact`
  - `epoch_binding_exact`
  - `typed_binding_identity_present`
- To: `Stopped`

### `PrepareBindingsInitializing`
- From: `Initializing`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation, runtime_epoch_id, session_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_absent_or_same`
  - `fence_binding_absent_or_same`
  - `generation_binding_absent_or_same`
  - `epoch_binding_absent_or_same`
  - `typed_binding_identity_present`
  - `runtime_binding_not_already_exact`
- Emits: `RuntimeBound`
- To: `Initializing`

### `PrepareBindingsIdle`
- From: `Idle`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation, runtime_epoch_id, session_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_absent_or_same`
  - `fence_binding_absent_or_same`
  - `generation_binding_absent_or_same`
  - `epoch_binding_absent_or_same`
  - `typed_binding_identity_present`
  - `runtime_binding_not_already_exact`
- Emits: `RuntimeBound`
- To: `Attached`

### `PrepareBindingsAttached`
- From: `Attached`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation, runtime_epoch_id, session_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_absent_or_same`
  - `fence_binding_absent_or_same`
  - `generation_binding_absent_or_same`
  - `epoch_binding_absent_or_same`
  - `typed_binding_identity_present`
  - `runtime_binding_not_already_exact`
- Emits: `RuntimeBound`
- To: `Attached`

### `PrepareBindingsRunning`
- From: `Running`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation, runtime_epoch_id, session_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_absent_or_same`
  - `fence_binding_absent_or_same`
  - `generation_binding_absent_or_same`
  - `epoch_binding_absent_or_same`
  - `typed_binding_identity_present`
  - `runtime_binding_not_already_exact`
- Emits: `RuntimeBound`
- To: `Running`

### `PrepareBindingsRetired`
- From: `Retired`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation, runtime_epoch_id, session_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_absent_or_same`
  - `fence_binding_absent_or_same`
  - `generation_binding_absent_or_same`
  - `epoch_binding_absent_or_same`
  - `typed_binding_identity_present`
  - `runtime_binding_not_already_exact`
- Emits: `RuntimeBound`
- To: `Retired`

### `PrepareBindingsStopped`
- From: `Stopped`
- On: `PrepareBindings`(agent_runtime_id, fence_token, generation, runtime_epoch_id, session_id)
- Guards:
  - `session_matches_current`
  - `runtime_binding_absent_or_same`
  - `fence_binding_absent_or_same`
  - `generation_binding_absent_or_same`
  - `epoch_binding_absent_or_same`
  - `typed_binding_identity_present`
  - `runtime_binding_not_already_exact`
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

### `ResolvePeerIngressReceiveClosedIdle`
- From: `Idle`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_closed`
- Emits: `PeerIngressReceiveResolved`
- To: `Idle`

### `ResolvePeerIngressReceiveClosedAttached`
- From: `Attached`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_closed`
- Emits: `PeerIngressReceiveResolved`
- To: `Attached`

### `ResolvePeerIngressReceiveClosedRunning`
- From: `Running`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_closed`
- Emits: `PeerIngressReceiveResolved`
- To: `Running`

### `ResolvePeerIngressReceiveClosedRetired`
- From: `Retired`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_closed`
- Emits: `PeerIngressReceiveResolved`
- To: `Retired`

### `ResolvePeerIngressReceiveClosedStopped`
- From: `Stopped`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_closed`
- Emits: `PeerIngressReceiveResolved`
- To: `Stopped`

### `ResolvePeerIngressReceiveFullIdle`
- From: `Idle`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_full`
- Emits: `PeerIngressReceiveResolved`
- To: `Idle`

### `ResolvePeerIngressReceiveFullAttached`
- From: `Attached`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_full`
- Emits: `PeerIngressReceiveResolved`
- To: `Attached`

### `ResolvePeerIngressReceiveFullRunning`
- From: `Running`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_full`
- Emits: `PeerIngressReceiveResolved`
- To: `Running`

### `ResolvePeerIngressReceiveFullRetired`
- From: `Retired`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_full`
- Emits: `PeerIngressReceiveResolved`
- To: `Retired`

### `ResolvePeerIngressReceiveFullStopped`
- From: `Stopped`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_full`
- Emits: `PeerIngressReceiveResolved`
- To: `Stopped`

### `ResolvePeerIngressReceivePlainEventIdle`
- From: `Idle`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `plain_event`
- Emits: `PeerIngressReceiveResolved`
- To: `Idle`

### `ResolvePeerIngressReceivePlainEventAttached`
- From: `Attached`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `plain_event`
- Emits: `PeerIngressReceiveResolved`
- To: `Attached`

### `ResolvePeerIngressReceivePlainEventRunning`
- From: `Running`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `plain_event`
- Emits: `PeerIngressReceiveResolved`
- To: `Running`

### `ResolvePeerIngressReceivePlainEventRetired`
- From: `Retired`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `plain_event`
- Emits: `PeerIngressReceiveResolved`
- To: `Retired`

### `ResolvePeerIngressReceivePlainEventStopped`
- From: `Stopped`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `plain_event`
- Emits: `PeerIngressReceiveResolved`
- To: `Stopped`

### `ResolvePeerIngressReceiveTrustedIdle`
- From: `Idle`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `trusted_sender`
- Emits: `PeerIngressReceiveResolved`
- To: `Idle`

### `ResolvePeerIngressReceiveTrustedAttached`
- From: `Attached`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `trusted_sender`
- Emits: `PeerIngressReceiveResolved`
- To: `Attached`

### `ResolvePeerIngressReceiveTrustedRunning`
- From: `Running`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `trusted_sender`
- Emits: `PeerIngressReceiveResolved`
- To: `Running`

### `ResolvePeerIngressReceiveTrustedRetired`
- From: `Retired`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `trusted_sender`
- Emits: `PeerIngressReceiveResolved`
- To: `Retired`

### `ResolvePeerIngressReceiveTrustedStopped`
- From: `Stopped`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `trusted_sender`
- Emits: `PeerIngressReceiveResolved`
- To: `Stopped`

### `ResolvePeerIngressReceiveAuthExemptUntrustedIdle`
- From: `Idle`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `untrusted_sender`
  - `auth_exempt`
- Emits: `PeerIngressReceiveResolved`
- To: `Idle`

### `ResolvePeerIngressReceiveAuthExemptUntrustedAttached`
- From: `Attached`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `untrusted_sender`
  - `auth_exempt`
- Emits: `PeerIngressReceiveResolved`
- To: `Attached`

### `ResolvePeerIngressReceiveAuthExemptUntrustedRunning`
- From: `Running`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `untrusted_sender`
  - `auth_exempt`
- Emits: `PeerIngressReceiveResolved`
- To: `Running`

### `ResolvePeerIngressReceiveAuthExemptUntrustedRetired`
- From: `Retired`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `untrusted_sender`
  - `auth_exempt`
- Emits: `PeerIngressReceiveResolved`
- To: `Retired`

### `ResolvePeerIngressReceiveAuthExemptUntrustedStopped`
- From: `Stopped`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `untrusted_sender`
  - `auth_exempt`
- Emits: `PeerIngressReceiveResolved`
- To: `Stopped`

### `ResolvePeerIngressReceiveAuthOpenUntrustedIdle`
- From: `Idle`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `untrusted_sender`
  - `auth_required_disabled`
  - `auth_not_exempt`
- Emits: `PeerIngressReceiveResolved`
- To: `Idle`

### `ResolvePeerIngressReceiveAuthOpenUntrustedAttached`
- From: `Attached`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `untrusted_sender`
  - `auth_required_disabled`
  - `auth_not_exempt`
- Emits: `PeerIngressReceiveResolved`
- To: `Attached`

### `ResolvePeerIngressReceiveAuthOpenUntrustedRunning`
- From: `Running`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `untrusted_sender`
  - `auth_required_disabled`
  - `auth_not_exempt`
- Emits: `PeerIngressReceiveResolved`
- To: `Running`

### `ResolvePeerIngressReceiveAuthOpenUntrustedRetired`
- From: `Retired`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `untrusted_sender`
  - `auth_required_disabled`
  - `auth_not_exempt`
- Emits: `PeerIngressReceiveResolved`
- To: `Retired`

### `ResolvePeerIngressReceiveAuthOpenUntrustedStopped`
- From: `Stopped`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `untrusted_sender`
  - `auth_required_disabled`
  - `auth_not_exempt`
- Emits: `PeerIngressReceiveResolved`
- To: `Stopped`

### `ResolvePeerIngressReceiveUntrustedQueuedDropIdle`
- From: `Idle`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `untrusted_sender`
  - `auth_required`
  - `auth_not_exempt`
  - `queued_work_present`
- Emits: `PeerIngressReceiveResolved`
- To: `Idle`

### `ResolvePeerIngressReceiveUntrustedQueuedDropAttached`
- From: `Attached`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `untrusted_sender`
  - `auth_required`
  - `auth_not_exempt`
  - `queued_work_present`
- Emits: `PeerIngressReceiveResolved`
- To: `Attached`

### `ResolvePeerIngressReceiveUntrustedQueuedDropRunning`
- From: `Running`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `untrusted_sender`
  - `auth_required`
  - `auth_not_exempt`
  - `queued_work_present`
- Emits: `PeerIngressReceiveResolved`
- To: `Running`

### `ResolvePeerIngressReceiveUntrustedQueuedDropRetired`
- From: `Retired`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `untrusted_sender`
  - `auth_required`
  - `auth_not_exempt`
  - `queued_work_present`
- Emits: `PeerIngressReceiveResolved`
- To: `Retired`

### `ResolvePeerIngressReceiveUntrustedQueuedDropStopped`
- From: `Stopped`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `untrusted_sender`
  - `auth_required`
  - `auth_not_exempt`
  - `queued_work_present`
- Emits: `PeerIngressReceiveResolved`
- To: `Stopped`

### `ResolvePeerIngressReceiveUntrustedEmptyDropIdle`
- From: `Idle`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `untrusted_sender`
  - `auth_required`
  - `auth_not_exempt`
  - `queued_work_empty`
- Emits: `PeerIngressReceiveResolved`
- To: `Idle`

### `ResolvePeerIngressReceiveUntrustedEmptyDropAttached`
- From: `Attached`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `untrusted_sender`
  - `auth_required`
  - `auth_not_exempt`
  - `queued_work_empty`
- Emits: `PeerIngressReceiveResolved`
- To: `Attached`

### `ResolvePeerIngressReceiveUntrustedEmptyDropRunning`
- From: `Running`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `untrusted_sender`
  - `auth_required`
  - `auth_not_exempt`
  - `queued_work_empty`
- Emits: `PeerIngressReceiveResolved`
- To: `Running`

### `ResolvePeerIngressReceiveUntrustedEmptyDropRetired`
- From: `Retired`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `untrusted_sender`
  - `auth_required`
  - `auth_not_exempt`
  - `queued_work_empty`
- Emits: `PeerIngressReceiveResolved`
- To: `Retired`

### `ResolvePeerIngressReceiveUntrustedEmptyDropStopped`
- From: `Stopped`
- On: `ResolvePeerIngressReceive`(kind, auth_required, auth_exempt, trusted, queued_work_present, queue_closed, queue_capacity_available)
- Guards:
  - `session_registered`
  - `queue_open`
  - `queue_capacity_available`
  - `external_entry`
  - `untrusted_sender`
  - `auth_required`
  - `auth_not_exempt`
  - `queued_work_empty`
- Emits: `PeerIngressReceiveResolved`
- To: `Stopped`

### `ResolvePeerIngressDequeuePlainEventIdle`
- From: `Idle`
- On: `ResolvePeerIngressDequeue`(kind, auth, queued_work_remaining)
- Guards:
  - `session_registered`
  - `plain_event`
- Emits: `PeerIngressDequeueResolved`
- To: `Idle`

### `ResolvePeerIngressDequeuePlainEventAttached`
- From: `Attached`
- On: `ResolvePeerIngressDequeue`(kind, auth, queued_work_remaining)
- Guards:
  - `session_registered`
  - `plain_event`
- Emits: `PeerIngressDequeueResolved`
- To: `Attached`

### `ResolvePeerIngressDequeuePlainEventRunning`
- From: `Running`
- On: `ResolvePeerIngressDequeue`(kind, auth, queued_work_remaining)
- Guards:
  - `session_registered`
  - `plain_event`
- Emits: `PeerIngressDequeueResolved`
- To: `Running`

### `ResolvePeerIngressDequeuePlainEventRetired`
- From: `Retired`
- On: `ResolvePeerIngressDequeue`(kind, auth, queued_work_remaining)
- Guards:
  - `session_registered`
  - `plain_event`
- Emits: `PeerIngressDequeueResolved`
- To: `Retired`

### `ResolvePeerIngressDequeuePlainEventStopped`
- From: `Stopped`
- On: `ResolvePeerIngressDequeue`(kind, auth, queued_work_remaining)
- Guards:
  - `session_registered`
  - `plain_event`
- Emits: `PeerIngressDequeueResolved`
- To: `Stopped`

### `ResolvePeerIngressDequeueAuthExemptExternalIdle`
- From: `Idle`
- On: `ResolvePeerIngressDequeue`(kind, auth, queued_work_remaining)
- Guards:
  - `session_registered`
  - `external_entry`
  - `auth_exempt`
- Emits: `PeerIngressDequeueResolved`
- To: `Idle`

### `ResolvePeerIngressDequeueAuthExemptExternalAttached`
- From: `Attached`
- On: `ResolvePeerIngressDequeue`(kind, auth, queued_work_remaining)
- Guards:
  - `session_registered`
  - `external_entry`
  - `auth_exempt`
- Emits: `PeerIngressDequeueResolved`
- To: `Attached`

### `ResolvePeerIngressDequeueAuthExemptExternalRunning`
- From: `Running`
- On: `ResolvePeerIngressDequeue`(kind, auth, queued_work_remaining)
- Guards:
  - `session_registered`
  - `external_entry`
  - `auth_exempt`
- Emits: `PeerIngressDequeueResolved`
- To: `Running`

### `ResolvePeerIngressDequeueAuthExemptExternalRetired`
- From: `Retired`
- On: `ResolvePeerIngressDequeue`(kind, auth, queued_work_remaining)
- Guards:
  - `session_registered`
  - `external_entry`
  - `auth_exempt`
- Emits: `PeerIngressDequeueResolved`
- To: `Retired`

### `ResolvePeerIngressDequeueAuthExemptExternalStopped`
- From: `Stopped`
- On: `ResolvePeerIngressDequeue`(kind, auth, queued_work_remaining)
- Guards:
  - `session_registered`
  - `external_entry`
  - `auth_exempt`
- Emits: `PeerIngressDequeueResolved`
- To: `Stopped`

### `ResolvePeerIngressDequeueRequiredRemainingIdle`
- From: `Idle`
- On: `ResolvePeerIngressDequeue`(kind, auth, queued_work_remaining)
- Guards:
  - `session_registered`
  - `external_entry`
  - `auth_required`
  - `queued_work_remaining`
- Emits: `PeerIngressDequeueResolved`
- To: `Idle`

### `ResolvePeerIngressDequeueRequiredRemainingAttached`
- From: `Attached`
- On: `ResolvePeerIngressDequeue`(kind, auth, queued_work_remaining)
- Guards:
  - `session_registered`
  - `external_entry`
  - `auth_required`
  - `queued_work_remaining`
- Emits: `PeerIngressDequeueResolved`
- To: `Attached`

### `ResolvePeerIngressDequeueRequiredRemainingRunning`
- From: `Running`
- On: `ResolvePeerIngressDequeue`(kind, auth, queued_work_remaining)
- Guards:
  - `session_registered`
  - `external_entry`
  - `auth_required`
  - `queued_work_remaining`
- Emits: `PeerIngressDequeueResolved`
- To: `Running`

### `ResolvePeerIngressDequeueRequiredRemainingRetired`
- From: `Retired`
- On: `ResolvePeerIngressDequeue`(kind, auth, queued_work_remaining)
- Guards:
  - `session_registered`
  - `external_entry`
  - `auth_required`
  - `queued_work_remaining`
- Emits: `PeerIngressDequeueResolved`
- To: `Retired`

### `ResolvePeerIngressDequeueRequiredRemainingStopped`
- From: `Stopped`
- On: `ResolvePeerIngressDequeue`(kind, auth, queued_work_remaining)
- Guards:
  - `session_registered`
  - `external_entry`
  - `auth_required`
  - `queued_work_remaining`
- Emits: `PeerIngressDequeueResolved`
- To: `Stopped`

### `ResolvePeerIngressDequeueRequiredEmptyIdle`
- From: `Idle`
- On: `ResolvePeerIngressDequeue`(kind, auth, queued_work_remaining)
- Guards:
  - `session_registered`
  - `external_entry`
  - `auth_required`
  - `queued_work_empty`
- Emits: `PeerIngressDequeueResolved`
- To: `Idle`

### `ResolvePeerIngressDequeueRequiredEmptyAttached`
- From: `Attached`
- On: `ResolvePeerIngressDequeue`(kind, auth, queued_work_remaining)
- Guards:
  - `session_registered`
  - `external_entry`
  - `auth_required`
  - `queued_work_empty`
- Emits: `PeerIngressDequeueResolved`
- To: `Attached`

### `ResolvePeerIngressDequeueRequiredEmptyRunning`
- From: `Running`
- On: `ResolvePeerIngressDequeue`(kind, auth, queued_work_remaining)
- Guards:
  - `session_registered`
  - `external_entry`
  - `auth_required`
  - `queued_work_empty`
- Emits: `PeerIngressDequeueResolved`
- To: `Running`

### `ResolvePeerIngressDequeueRequiredEmptyRetired`
- From: `Retired`
- On: `ResolvePeerIngressDequeue`(kind, auth, queued_work_remaining)
- Guards:
  - `session_registered`
  - `external_entry`
  - `auth_required`
  - `queued_work_empty`
- Emits: `PeerIngressDequeueResolved`
- To: `Retired`

### `ResolvePeerIngressDequeueRequiredEmptyStopped`
- From: `Stopped`
- On: `ResolvePeerIngressDequeue`(kind, auth, queued_work_remaining)
- Guards:
  - `session_registered`
  - `external_entry`
  - `auth_required`
  - `queued_work_empty`
- Emits: `PeerIngressDequeueResolved`
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

### `ResolveUserInterruptPublicResultAcceptedInitializing`
- From: `Initializing`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `accepted`
- Emits: `UserInterruptPublicResultResolved`
- To: `Initializing`

### `ResolveUserInterruptPublicResultAcceptedIdle`
- From: `Idle`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `accepted`
- Emits: `UserInterruptPublicResultResolved`
- To: `Idle`

### `ResolveUserInterruptPublicResultAcceptedAttached`
- From: `Attached`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `accepted`
- Emits: `UserInterruptPublicResultResolved`
- To: `Attached`

### `ResolveUserInterruptPublicResultAcceptedRunning`
- From: `Running`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `accepted`
- Emits: `UserInterruptPublicResultResolved`
- To: `Running`

### `ResolveUserInterruptPublicResultAcceptedRetired`
- From: `Retired`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `accepted`
- Emits: `UserInterruptPublicResultResolved`
- To: `Retired`

### `ResolveUserInterruptPublicResultAcceptedStopped`
- From: `Stopped`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `accepted`
- Emits: `UserInterruptPublicResultResolved`
- To: `Stopped`

### `ResolveUserInterruptPublicResultNoopInitializing`
- From: `Initializing`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `noop_state`
- Emits: `UserInterruptPublicResultResolved`
- To: `Initializing`

### `ResolveUserInterruptPublicResultNoopIdle`
- From: `Idle`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `noop_state`
- Emits: `UserInterruptPublicResultResolved`
- To: `Idle`

### `ResolveUserInterruptPublicResultNoopAttached`
- From: `Attached`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `noop_state`
- Emits: `UserInterruptPublicResultResolved`
- To: `Attached`

### `ResolveUserInterruptPublicResultNoopRunning`
- From: `Running`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `noop_state`
- Emits: `UserInterruptPublicResultResolved`
- To: `Running`

### `ResolveUserInterruptPublicResultNoopRetired`
- From: `Retired`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `noop_state`
- Emits: `UserInterruptPublicResultResolved`
- To: `Retired`

### `ResolveUserInterruptPublicResultNoopStopped`
- From: `Stopped`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `noop_state`
- Emits: `UserInterruptPublicResultResolved`
- To: `Stopped`

### `ResolveUserInterruptPublicResultStagedNoopInitializing`
- From: `Initializing`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `staged_noop_state`
- Emits: `UserInterruptPublicResultResolved`
- To: `Initializing`

### `ResolveUserInterruptPublicResultStagedNoopIdle`
- From: `Idle`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `staged_noop_state`
- Emits: `UserInterruptPublicResultResolved`
- To: `Idle`

### `ResolveUserInterruptPublicResultStagedNoopAttached`
- From: `Attached`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `staged_noop_state`
- Emits: `UserInterruptPublicResultResolved`
- To: `Attached`

### `ResolveUserInterruptPublicResultStagedNoopRunning`
- From: `Running`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `staged_noop_state`
- Emits: `UserInterruptPublicResultResolved`
- To: `Running`

### `ResolveUserInterruptPublicResultStagedNoopRetired`
- From: `Retired`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `staged_noop_state`
- Emits: `UserInterruptPublicResultResolved`
- To: `Retired`

### `ResolveUserInterruptPublicResultStagedNoopStopped`
- From: `Stopped`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `staged_noop_state`
- Emits: `UserInterruptPublicResultResolved`
- To: `Stopped`

### `ResolveUserInterruptPublicResultDestroyedPresentInitializing`
- From: `Initializing`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `destroyed`
  - `target_present`
- Emits: `UserInterruptPublicResultResolved`
- To: `Initializing`

### `ResolveUserInterruptPublicResultDestroyedPresentIdle`
- From: `Idle`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `destroyed`
  - `target_present`
- Emits: `UserInterruptPublicResultResolved`
- To: `Idle`

### `ResolveUserInterruptPublicResultDestroyedPresentAttached`
- From: `Attached`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `destroyed`
  - `target_present`
- Emits: `UserInterruptPublicResultResolved`
- To: `Attached`

### `ResolveUserInterruptPublicResultDestroyedPresentRunning`
- From: `Running`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `destroyed`
  - `target_present`
- Emits: `UserInterruptPublicResultResolved`
- To: `Running`

### `ResolveUserInterruptPublicResultDestroyedPresentRetired`
- From: `Retired`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `destroyed`
  - `target_present`
- Emits: `UserInterruptPublicResultResolved`
- To: `Retired`

### `ResolveUserInterruptPublicResultDestroyedPresentStopped`
- From: `Stopped`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `destroyed`
  - `target_present`
- Emits: `UserInterruptPublicResultResolved`
- To: `Stopped`

### `ResolveUserInterruptPublicResultDestroyedMissingInitializing`
- From: `Initializing`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `destroyed`
  - `target_missing`
- Emits: `UserInterruptPublicResultResolved`
- To: `Initializing`

### `ResolveUserInterruptPublicResultDestroyedMissingIdle`
- From: `Idle`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `destroyed`
  - `target_missing`
- Emits: `UserInterruptPublicResultResolved`
- To: `Idle`

### `ResolveUserInterruptPublicResultDestroyedMissingAttached`
- From: `Attached`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `destroyed`
  - `target_missing`
- Emits: `UserInterruptPublicResultResolved`
- To: `Attached`

### `ResolveUserInterruptPublicResultDestroyedMissingRunning`
- From: `Running`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `destroyed`
  - `target_missing`
- Emits: `UserInterruptPublicResultResolved`
- To: `Running`

### `ResolveUserInterruptPublicResultDestroyedMissingRetired`
- From: `Retired`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `destroyed`
  - `target_missing`
- Emits: `UserInterruptPublicResultResolved`
- To: `Retired`

### `ResolveUserInterruptPublicResultDestroyedMissingStopped`
- From: `Stopped`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `destroyed`
  - `target_missing`
- Emits: `UserInterruptPublicResultResolved`
- To: `Stopped`

### `ResolveUserInterruptPublicResultPromotingConflictInitializing`
- From: `Initializing`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_accepted`
  - `promoting`
- Emits: `UserInterruptPublicResultResolved`
- To: `Initializing`

### `ResolveUserInterruptPublicResultPromotingConflictIdle`
- From: `Idle`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_accepted`
  - `promoting`
- Emits: `UserInterruptPublicResultResolved`
- To: `Idle`

### `ResolveUserInterruptPublicResultPromotingConflictAttached`
- From: `Attached`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_accepted`
  - `promoting`
- Emits: `UserInterruptPublicResultResolved`
- To: `Attached`

### `ResolveUserInterruptPublicResultPromotingConflictRunning`
- From: `Running`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_accepted`
  - `promoting`
- Emits: `UserInterruptPublicResultResolved`
- To: `Running`

### `ResolveUserInterruptPublicResultPromotingConflictRetired`
- From: `Retired`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_accepted`
  - `promoting`
- Emits: `UserInterruptPublicResultResolved`
- To: `Retired`

### `ResolveUserInterruptPublicResultPromotingConflictStopped`
- From: `Stopped`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_accepted`
  - `promoting`
- Emits: `UserInterruptPublicResultResolved`
- To: `Stopped`

### `ResolveUserInterruptPublicResultNotInterruptibleConflictInitializing`
- From: `Initializing`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `not_interruptible`
- Emits: `UserInterruptPublicResultResolved`
- To: `Initializing`

### `ResolveUserInterruptPublicResultNotInterruptibleConflictIdle`
- From: `Idle`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `not_interruptible`
- Emits: `UserInterruptPublicResultResolved`
- To: `Idle`

### `ResolveUserInterruptPublicResultNotInterruptibleConflictAttached`
- From: `Attached`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `not_interruptible`
- Emits: `UserInterruptPublicResultResolved`
- To: `Attached`

### `ResolveUserInterruptPublicResultNotInterruptibleConflictRunning`
- From: `Running`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `not_interruptible`
- Emits: `UserInterruptPublicResultResolved`
- To: `Running`

### `ResolveUserInterruptPublicResultNotInterruptibleConflictRetired`
- From: `Retired`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `not_interruptible`
- Emits: `UserInterruptPublicResultResolved`
- To: `Retired`

### `ResolveUserInterruptPublicResultNotInterruptibleConflictStopped`
- From: `Stopped`
- On: `ResolveUserInterruptPublicResult`(observation, target_present, staged_promotion_busy)
- Guards:
  - `not_promoting`
  - `not_interruptible`
- Emits: `UserInterruptPublicResultResolved`
- To: `Stopped`

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
  - `visibility_authority_matches_machine_catalog`
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
  - `visibility_authority_matches_machine_catalog`
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
  - `visibility_authority_matches_machine_catalog`
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
  - `visibility_authority_matches_machine_catalog`
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
  - `visibility_authority_matches_machine_catalog`
- Emits: `CommittedVisibleSetPublished`
- To: `Stopped`

### `RetireRequestedFromIdle`
- From: `Idle`, `Attached`, `Running`
- On: `Retire`(session_id)
- Guards:
  - `runtime_binding_present`
- Emits: `RuntimeRetired`
- To: `Retired`

### `RetireRequestedFromIdleUnbound`
- From: `Idle`, `Attached`, `Running`
- On: `Retire`(session_id)
- Guards:
  - `runtime_binding_absent`
- To: `Retired`

### `RetireAlreadyRetired`
- From: `Retired`
- On: `Retire`(session_id)
- Guards:
  - `runtime_binding_present`
- Emits: `RuntimeRetired`
- To: `Retired`

### `RetireAlreadyRetiredUnbound`
- From: `Retired`
- On: `Retire`(session_id)
- Guards:
  - `runtime_binding_absent`
- To: `Retired`

### `Reset`
- From: `Initializing`, `Idle`, `Attached`, `Retired`
- On: `Reset`()
- Emits: `RuntimeNotice`
- To: `Idle`

### `StopRuntimeExecutorInitializing`
- From: `Initializing`
- On: `StopRuntimeExecutor`(reason)
- Emits: `RuntimeNotice`, `RuntimeEffectFact`
- To: `Initializing`

### `StopRuntimeExecutorIdle`
- From: `Idle`
- On: `StopRuntimeExecutor`(reason)
- Emits: `RuntimeNotice`, `RuntimeEffectFact`
- To: `Idle`

### `StopRuntimeExecutorRetired`
- From: `Retired`
- On: `StopRuntimeExecutor`(reason)
- Emits: `RuntimeNotice`, `RuntimeEffectFact`
- To: `Retired`

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

### `ResolveRuntimeCompletionResultCompletedInitializing`
- From: `Initializing`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_run_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Initializing`

### `ResolveRuntimeCompletionResultCompletedIdle`
- From: `Idle`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_run_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Idle`

### `ResolveRuntimeCompletionResultCompletedAttached`
- From: `Attached`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_run_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Attached`

### `ResolveRuntimeCompletionResultCompletedRunning`
- From: `Running`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_run_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Running`

### `ResolveRuntimeCompletionResultCompletedRetired`
- From: `Retired`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_run_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Retired`

### `ResolveRuntimeCompletionResultCompletedStopped`
- From: `Stopped`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_run_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Stopped`

### `ResolveRuntimeCompletionResultWithoutResultInitializing`
- From: `Initializing`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_no_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Initializing`

### `ResolveRuntimeCompletionResultWithoutResultIdle`
- From: `Idle`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_no_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Idle`

### `ResolveRuntimeCompletionResultWithoutResultAttached`
- From: `Attached`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_no_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Attached`

### `ResolveRuntimeCompletionResultWithoutResultRunning`
- From: `Running`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_no_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Running`

### `ResolveRuntimeCompletionResultWithoutResultRetired`
- From: `Retired`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_no_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Retired`

### `ResolveRuntimeCompletionResultWithoutResultStopped`
- From: `Stopped`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_no_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Stopped`

### `ResolveRuntimeCompletionResultCallbackPendingInitializing`
- From: `Initializing`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_callback_pending`
- Emits: `RuntimeCompletionResultResolved`
- To: `Initializing`

### `ResolveRuntimeCompletionResultCallbackPendingIdle`
- From: `Idle`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_callback_pending`
- Emits: `RuntimeCompletionResultResolved`
- To: `Idle`

### `ResolveRuntimeCompletionResultCallbackPendingAttached`
- From: `Attached`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_callback_pending`
- Emits: `RuntimeCompletionResultResolved`
- To: `Attached`

### `ResolveRuntimeCompletionResultCallbackPendingRunning`
- From: `Running`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_callback_pending`
- Emits: `RuntimeCompletionResultResolved`
- To: `Running`

### `ResolveRuntimeCompletionResultCallbackPendingRetired`
- From: `Retired`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_callback_pending`
- Emits: `RuntimeCompletionResultResolved`
- To: `Retired`

### `ResolveRuntimeCompletionResultCallbackPendingStopped`
- From: `Stopped`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_callback_pending`
- Emits: `RuntimeCompletionResultResolved`
- To: `Stopped`

### `ResolveRuntimeCompletionResultCancelledInitializing`
- From: `Initializing`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_machine`
  - `machine_cancelled`
- Emits: `RuntimeCompletionResultResolved`
- To: `Initializing`

### `ResolveRuntimeCompletionResultCancelledIdle`
- From: `Idle`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_machine`
  - `machine_cancelled`
- Emits: `RuntimeCompletionResultResolved`
- To: `Idle`

### `ResolveRuntimeCompletionResultCancelledAttached`
- From: `Attached`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_machine`
  - `machine_cancelled`
- Emits: `RuntimeCompletionResultResolved`
- To: `Attached`

### `ResolveRuntimeCompletionResultCancelledRunning`
- From: `Running`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_machine`
  - `machine_cancelled`
- Emits: `RuntimeCompletionResultResolved`
- To: `Running`

### `ResolveRuntimeCompletionResultCancelledRetired`
- From: `Retired`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_machine`
  - `machine_cancelled`
- Emits: `RuntimeCompletionResultResolved`
- To: `Retired`

### `ResolveRuntimeCompletionResultCancelledStopped`
- From: `Stopped`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_machine`
  - `machine_cancelled`
- Emits: `RuntimeCompletionResultResolved`
- To: `Stopped`

### `ResolveRuntimeCompletionResultRuntimeApplyFailedInitializing`
- From: `Initializing`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_machine`
  - `machine_failed`
  - `machine_failure_cause_known`
  - `machine_runtime_apply_failed`
- Emits: `RuntimeCompletionResultResolved`
- To: `Initializing`

### `ResolveRuntimeCompletionResultRuntimeApplyFailedIdle`
- From: `Idle`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_machine`
  - `machine_failed`
  - `machine_failure_cause_known`
  - `machine_runtime_apply_failed`
- Emits: `RuntimeCompletionResultResolved`
- To: `Idle`

### `ResolveRuntimeCompletionResultRuntimeApplyFailedAttached`
- From: `Attached`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_machine`
  - `machine_failed`
  - `machine_failure_cause_known`
  - `machine_runtime_apply_failed`
- Emits: `RuntimeCompletionResultResolved`
- To: `Attached`

### `ResolveRuntimeCompletionResultRuntimeApplyFailedRunning`
- From: `Running`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_machine`
  - `machine_failed`
  - `machine_failure_cause_known`
  - `machine_runtime_apply_failed`
- Emits: `RuntimeCompletionResultResolved`
- To: `Running`

### `ResolveRuntimeCompletionResultRuntimeApplyFailedRetired`
- From: `Retired`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_machine`
  - `machine_failed`
  - `machine_failure_cause_known`
  - `machine_runtime_apply_failed`
- Emits: `RuntimeCompletionResultResolved`
- To: `Retired`

### `ResolveRuntimeCompletionResultRuntimeApplyFailedStopped`
- From: `Stopped`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_machine`
  - `machine_failed`
  - `machine_failure_cause_known`
  - `machine_runtime_apply_failed`
- Emits: `RuntimeCompletionResultResolved`
- To: `Stopped`

### `ResolveRuntimeCompletionResultMachineFailedInitializing`
- From: `Initializing`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_machine`
  - `machine_failed`
  - `machine_failure_cause_known`
  - `machine_not_runtime_apply_failed`
- Emits: `RuntimeCompletionResultResolved`
- To: `Initializing`

### `ResolveRuntimeCompletionResultMachineFailedIdle`
- From: `Idle`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_machine`
  - `machine_failed`
  - `machine_failure_cause_known`
  - `machine_not_runtime_apply_failed`
- Emits: `RuntimeCompletionResultResolved`
- To: `Idle`

### `ResolveRuntimeCompletionResultMachineFailedAttached`
- From: `Attached`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_machine`
  - `machine_failed`
  - `machine_failure_cause_known`
  - `machine_not_runtime_apply_failed`
- Emits: `RuntimeCompletionResultResolved`
- To: `Attached`

### `ResolveRuntimeCompletionResultMachineFailedRunning`
- From: `Running`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_machine`
  - `machine_failed`
  - `machine_failure_cause_known`
  - `machine_not_runtime_apply_failed`
- Emits: `RuntimeCompletionResultResolved`
- To: `Running`

### `ResolveRuntimeCompletionResultMachineFailedRetired`
- From: `Retired`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_machine`
  - `machine_failed`
  - `machine_failure_cause_known`
  - `machine_not_runtime_apply_failed`
- Emits: `RuntimeCompletionResultResolved`
- To: `Retired`

### `ResolveRuntimeCompletionResultMachineFailedStopped`
- From: `Stopped`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_succeeded`
  - `terminal_machine`
  - `machine_failed`
  - `machine_failure_cause_known`
  - `machine_not_runtime_apply_failed`
- Emits: `RuntimeCompletionResultResolved`
- To: `Stopped`

### `ResolveRuntimeCompletionResultFinalizationFailureWithResultInitializing`
- From: `Initializing`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_failed`
  - `terminal_run_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Initializing`

### `ResolveRuntimeCompletionResultFinalizationFailureWithResultIdle`
- From: `Idle`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_failed`
  - `terminal_run_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Idle`

### `ResolveRuntimeCompletionResultFinalizationFailureWithResultAttached`
- From: `Attached`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_failed`
  - `terminal_run_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Attached`

### `ResolveRuntimeCompletionResultFinalizationFailureWithResultRunning`
- From: `Running`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_failed`
  - `terminal_run_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Running`

### `ResolveRuntimeCompletionResultFinalizationFailureWithResultRetired`
- From: `Retired`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_failed`
  - `terminal_run_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Retired`

### `ResolveRuntimeCompletionResultFinalizationFailureWithResultStopped`
- From: `Stopped`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_failed`
  - `terminal_run_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Stopped`

### `ResolveRuntimeCompletionResultFinalizationFailureWithoutResultInitializing`
- From: `Initializing`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_failed`
  - `terminal_without_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Initializing`

### `ResolveRuntimeCompletionResultFinalizationFailureWithoutResultIdle`
- From: `Idle`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_failed`
  - `terminal_without_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Idle`

### `ResolveRuntimeCompletionResultFinalizationFailureWithoutResultAttached`
- From: `Attached`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_failed`
  - `terminal_without_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Attached`

### `ResolveRuntimeCompletionResultFinalizationFailureWithoutResultRunning`
- From: `Running`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_failed`
  - `terminal_without_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Running`

### `ResolveRuntimeCompletionResultFinalizationFailureWithoutResultRetired`
- From: `Retired`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_failed`
  - `terminal_without_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Retired`

### `ResolveRuntimeCompletionResultFinalizationFailureWithoutResultStopped`
- From: `Stopped`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `run_correlated`
  - `finalization_failed`
  - `terminal_without_result`
- Emits: `RuntimeCompletionResultResolved`
- To: `Stopped`

### `ResolveRuntimeCompletionResultRuntimeTerminatedInitializing`
- From: `Initializing`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `no_run_result`
  - `finalization_succeeded`
  - `terminal_runtime_terminated`
- Emits: `RuntimeCompletionResultResolved`
- To: `Initializing`

### `ResolveRuntimeCompletionResultRuntimeTerminatedIdle`
- From: `Idle`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `no_run_result`
  - `finalization_succeeded`
  - `terminal_runtime_terminated`
- Emits: `RuntimeCompletionResultResolved`
- To: `Idle`

### `ResolveRuntimeCompletionResultRuntimeTerminatedAttached`
- From: `Attached`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `no_run_result`
  - `finalization_succeeded`
  - `terminal_runtime_terminated`
- Emits: `RuntimeCompletionResultResolved`
- To: `Attached`

### `ResolveRuntimeCompletionResultRuntimeTerminatedRunning`
- From: `Running`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `no_run_result`
  - `finalization_succeeded`
  - `terminal_runtime_terminated`
- Emits: `RuntimeCompletionResultResolved`
- To: `Running`

### `ResolveRuntimeCompletionResultRuntimeTerminatedRetired`
- From: `Retired`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `no_run_result`
  - `finalization_succeeded`
  - `terminal_runtime_terminated`
- Emits: `RuntimeCompletionResultResolved`
- To: `Retired`

### `ResolveRuntimeCompletionResultRuntimeTerminatedStopped`
- From: `Stopped`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `no_run_result`
  - `finalization_succeeded`
  - `terminal_runtime_terminated`
- Emits: `RuntimeCompletionResultResolved`
- To: `Stopped`

### `ResolveRuntimeCompletionResultRuntimeTerminatedDestroyedDestroyed`
- From: `Destroyed`
- On: `ResolveRuntimeCompletionResult`(run_id, terminal, finalization)
- Guards:
  - `session_registered`
  - `no_run_result`
  - `finalization_succeeded`
  - `terminal_runtime_terminated`
- Emits: `RuntimeCompletionResultResolved`
- To: `Destroyed`

### `ResolveRuntimeCompletionCleanupArchivedInitializing`
- From: `Initializing`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `archived_by_generated_authority`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Initializing`

### `ResolveRuntimeCompletionCleanupArchivedIdle`
- From: `Idle`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `archived_by_generated_authority`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Idle`

### `ResolveRuntimeCompletionCleanupArchivedAttached`
- From: `Attached`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `archived_by_generated_authority`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Attached`

### `ResolveRuntimeCompletionCleanupArchivedRunning`
- From: `Running`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `archived_by_generated_authority`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Running`

### `ResolveRuntimeCompletionCleanupArchivedRetired`
- From: `Retired`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `archived_by_generated_authority`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Retired`

### `ResolveRuntimeCompletionCleanupArchivedStopped`
- From: `Stopped`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `archived_by_generated_authority`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Stopped`

### `ResolveRuntimeCompletionCleanupRuntimeTerminatedInitializing`
- From: `Initializing`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `runtime_terminated`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Initializing`

### `ResolveRuntimeCompletionCleanupRuntimeTerminatedIdle`
- From: `Idle`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `runtime_terminated`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Idle`

### `ResolveRuntimeCompletionCleanupRuntimeTerminatedAttached`
- From: `Attached`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `runtime_terminated`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Attached`

### `ResolveRuntimeCompletionCleanupRuntimeTerminatedRunning`
- From: `Running`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `runtime_terminated`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Running`

### `ResolveRuntimeCompletionCleanupRuntimeTerminatedRetired`
- From: `Retired`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `runtime_terminated`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Retired`

### `ResolveRuntimeCompletionCleanupRuntimeTerminatedStopped`
- From: `Stopped`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `runtime_terminated`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Stopped`

### `ResolveRuntimeCompletionCleanupFinalizationFailedInitializing`
- From: `Initializing`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `finalization_failed`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Initializing`

### `ResolveRuntimeCompletionCleanupFinalizationFailedIdle`
- From: `Idle`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `finalization_failed`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Idle`

### `ResolveRuntimeCompletionCleanupFinalizationFailedAttached`
- From: `Attached`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `finalization_failed`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Attached`

### `ResolveRuntimeCompletionCleanupFinalizationFailedRunning`
- From: `Running`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `finalization_failed`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Running`

### `ResolveRuntimeCompletionCleanupFinalizationFailedRetired`
- From: `Retired`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `finalization_failed`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Retired`

### `ResolveRuntimeCompletionCleanupFinalizationFailedStopped`
- From: `Stopped`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `finalization_failed`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Stopped`

### `ResolveRuntimeCompletionCleanupRuntimeApplyFailedInitializing`
- From: `Initializing`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `runtime_apply_failed`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Initializing`

### `ResolveRuntimeCompletionCleanupRuntimeApplyFailedIdle`
- From: `Idle`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `runtime_apply_failed`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Idle`

### `ResolveRuntimeCompletionCleanupRuntimeApplyFailedAttached`
- From: `Attached`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `runtime_apply_failed`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Attached`

### `ResolveRuntimeCompletionCleanupRuntimeApplyFailedRunning`
- From: `Running`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `runtime_apply_failed`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Running`

### `ResolveRuntimeCompletionCleanupRuntimeApplyFailedRetired`
- From: `Retired`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `runtime_apply_failed`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Retired`

### `ResolveRuntimeCompletionCleanupRuntimeApplyFailedStopped`
- From: `Stopped`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `runtime_apply_failed`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Stopped`

### `ResolveRuntimeCompletionCleanupAbandonedWithoutLiveSessionInitializing`
- From: `Initializing`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `abandoned`
  - `live_absent`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Initializing`

### `ResolveRuntimeCompletionCleanupAbandonedWithoutLiveSessionIdle`
- From: `Idle`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `abandoned`
  - `live_absent`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Idle`

### `ResolveRuntimeCompletionCleanupAbandonedWithoutLiveSessionAttached`
- From: `Attached`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `abandoned`
  - `live_absent`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Attached`

### `ResolveRuntimeCompletionCleanupAbandonedWithoutLiveSessionRunning`
- From: `Running`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `abandoned`
  - `live_absent`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Running`

### `ResolveRuntimeCompletionCleanupAbandonedWithoutLiveSessionRetired`
- From: `Retired`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `abandoned`
  - `live_absent`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Retired`

### `ResolveRuntimeCompletionCleanupAbandonedWithoutLiveSessionStopped`
- From: `Stopped`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `abandoned`
  - `live_absent`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Stopped`

### `ResolveRuntimeCompletionCleanupCancelledWithoutLiveSessionInitializing`
- From: `Initializing`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `cancelled`
  - `live_absent`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Initializing`

### `ResolveRuntimeCompletionCleanupCancelledWithoutLiveSessionIdle`
- From: `Idle`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `cancelled`
  - `live_absent`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Idle`

### `ResolveRuntimeCompletionCleanupCancelledWithoutLiveSessionAttached`
- From: `Attached`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `cancelled`
  - `live_absent`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Attached`

### `ResolveRuntimeCompletionCleanupCancelledWithoutLiveSessionRunning`
- From: `Running`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `cancelled`
  - `live_absent`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Running`

### `ResolveRuntimeCompletionCleanupCancelledWithoutLiveSessionRetired`
- From: `Retired`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `cancelled`
  - `live_absent`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Retired`

### `ResolveRuntimeCompletionCleanupCancelledWithoutLiveSessionStopped`
- From: `Stopped`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `cancelled`
  - `live_absent`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Stopped`

### `ResolveRuntimeCompletionCleanupRetainInitializing`
- From: `Initializing`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `cleanup_outcome_absent`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Initializing`

### `ResolveRuntimeCompletionCleanupRetainIdle`
- From: `Idle`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `cleanup_outcome_absent`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Idle`

### `ResolveRuntimeCompletionCleanupRetainAttached`
- From: `Attached`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `cleanup_outcome_absent`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Attached`

### `ResolveRuntimeCompletionCleanupRetainRunning`
- From: `Running`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `cleanup_outcome_absent`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Running`

### `ResolveRuntimeCompletionCleanupRetainRetired`
- From: `Retired`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `cleanup_outcome_absent`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Retired`

### `ResolveRuntimeCompletionCleanupRetainStopped`
- From: `Stopped`
- On: `ResolveRuntimeCompletionCleanup`(session_id, observation_session_id, observation_agent_runtime_id, observation_fence_token, observation_runtime_generation, observation_runtime_epoch_id, outcome, archived_by_authority, live_session)
- Guards:
  - `observation_matches_session`
  - `observation_matches_runtime_binding`
  - `not_archived`
  - `cleanup_outcome_absent`
- Emits: `RuntimeCompletionCleanupResolved`
- To: `Stopped`

### `ResolveRuntimeCompletionWaitFailureChannelClosedInitializing`
- From: `Initializing`
- On: `ResolveRuntimeCompletionWaitFailure`(session_id, failure)
- Guards:
  - `session_registered`
  - `session_matches`
  - `channel_closed`
- Emits: `RuntimeCompletionWaitFailureResolved`
- To: `Initializing`

### `ResolveRuntimeCompletionWaitFailureChannelClosedIdle`
- From: `Idle`
- On: `ResolveRuntimeCompletionWaitFailure`(session_id, failure)
- Guards:
  - `session_registered`
  - `session_matches`
  - `channel_closed`
- Emits: `RuntimeCompletionWaitFailureResolved`
- To: `Idle`

### `ResolveRuntimeCompletionWaitFailureChannelClosedAttached`
- From: `Attached`
- On: `ResolveRuntimeCompletionWaitFailure`(session_id, failure)
- Guards:
  - `session_registered`
  - `session_matches`
  - `channel_closed`
- Emits: `RuntimeCompletionWaitFailureResolved`
- To: `Attached`

### `ResolveRuntimeCompletionWaitFailureChannelClosedRunning`
- From: `Running`
- On: `ResolveRuntimeCompletionWaitFailure`(session_id, failure)
- Guards:
  - `session_registered`
  - `session_matches`
  - `channel_closed`
- Emits: `RuntimeCompletionWaitFailureResolved`
- To: `Running`

### `ResolveRuntimeCompletionWaitFailureChannelClosedRetired`
- From: `Retired`
- On: `ResolveRuntimeCompletionWaitFailure`(session_id, failure)
- Guards:
  - `session_registered`
  - `session_matches`
  - `channel_closed`
- Emits: `RuntimeCompletionWaitFailureResolved`
- To: `Retired`

### `ResolveRuntimeCompletionWaitFailureChannelClosedStopped`
- From: `Stopped`
- On: `ResolveRuntimeCompletionWaitFailure`(session_id, failure)
- Guards:
  - `session_registered`
  - `session_matches`
  - `channel_closed`
- Emits: `RuntimeCompletionWaitFailureResolved`
- To: `Stopped`

### `ResolveRuntimeCompletionWaitFailureAuthorityUnavailableInitializing`
- From: `Initializing`
- On: `ResolveRuntimeCompletionWaitFailure`(session_id, failure)
- Guards:
  - `session_registered`
  - `session_matches`
  - `authority_unavailable`
- Emits: `RuntimeCompletionWaitFailureResolved`
- To: `Initializing`

### `ResolveRuntimeCompletionWaitFailureAuthorityUnavailableIdle`
- From: `Idle`
- On: `ResolveRuntimeCompletionWaitFailure`(session_id, failure)
- Guards:
  - `session_registered`
  - `session_matches`
  - `authority_unavailable`
- Emits: `RuntimeCompletionWaitFailureResolved`
- To: `Idle`

### `ResolveRuntimeCompletionWaitFailureAuthorityUnavailableAttached`
- From: `Attached`
- On: `ResolveRuntimeCompletionWaitFailure`(session_id, failure)
- Guards:
  - `session_registered`
  - `session_matches`
  - `authority_unavailable`
- Emits: `RuntimeCompletionWaitFailureResolved`
- To: `Attached`

### `ResolveRuntimeCompletionWaitFailureAuthorityUnavailableRunning`
- From: `Running`
- On: `ResolveRuntimeCompletionWaitFailure`(session_id, failure)
- Guards:
  - `session_registered`
  - `session_matches`
  - `authority_unavailable`
- Emits: `RuntimeCompletionWaitFailureResolved`
- To: `Running`

### `ResolveRuntimeCompletionWaitFailureAuthorityUnavailableRetired`
- From: `Retired`
- On: `ResolveRuntimeCompletionWaitFailure`(session_id, failure)
- Guards:
  - `session_registered`
  - `session_matches`
  - `authority_unavailable`
- Emits: `RuntimeCompletionWaitFailureResolved`
- To: `Retired`

### `ResolveRuntimeCompletionWaitFailureAuthorityUnavailableStopped`
- From: `Stopped`
- On: `ResolveRuntimeCompletionWaitFailure`(session_id, failure)
- Guards:
  - `session_registered`
  - `session_matches`
  - `authority_unavailable`
- Emits: `RuntimeCompletionWaitFailureResolved`
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

### `RecoverRuntimeAuthorityInitializing`
- From: `Initializing`
- On: `RecoverRuntimeAuthority`(session_id, state, agent_runtime_id, fence_token, runtime_generation, runtime_epoch_id, current_run_id, pre_run_phase, silent_intent_overrides)
- Guards:
  - `observed_initializing`
  - `no_run_binding`
  - `fence_requires_runtime`
  - `generation_requires_runtime`
  - `epoch_requires_runtime`
  - `binding_identity_present_when_runtime_bound`
- To: `Initializing`

### `RecoverRuntimeAuthorityIdle`
- From: `Initializing`
- On: `RecoverRuntimeAuthority`(session_id, state, agent_runtime_id, fence_token, runtime_generation, runtime_epoch_id, current_run_id, pre_run_phase, silent_intent_overrides)
- Guards:
  - `observed_idle`
  - `no_run_binding`
  - `fence_requires_runtime`
  - `generation_requires_runtime`
  - `epoch_requires_runtime`
  - `binding_identity_present_when_runtime_bound`
- To: `Idle`

### `RecoverRuntimeAuthorityAttached`
- From: `Initializing`
- On: `RecoverRuntimeAuthority`(session_id, state, agent_runtime_id, fence_token, runtime_generation, runtime_epoch_id, current_run_id, pre_run_phase, silent_intent_overrides)
- Guards:
  - `observed_attached`
  - `no_run_binding`
  - `fence_requires_runtime`
  - `generation_requires_runtime`
  - `epoch_requires_runtime`
  - `binding_identity_present_when_runtime_bound`
- To: `Attached`

### `RecoverRuntimeAuthorityRunning`
- From: `Initializing`
- On: `RecoverRuntimeAuthority`(session_id, state, agent_runtime_id, fence_token, runtime_generation, runtime_epoch_id, current_run_id, pre_run_phase, silent_intent_overrides)
- Guards:
  - `observed_running`
  - `run_binding_complete`
  - `fence_requires_runtime`
  - `generation_requires_runtime`
  - `epoch_requires_runtime`
  - `binding_identity_present_when_runtime_bound`
- To: `Running`

### `RecoverRuntimeAuthorityRetired`
- From: `Initializing`
- On: `RecoverRuntimeAuthority`(session_id, state, agent_runtime_id, fence_token, runtime_generation, runtime_epoch_id, current_run_id, pre_run_phase, silent_intent_overrides)
- Guards:
  - `observed_retired`
  - `run_binding_pair`
  - `fence_requires_runtime`
  - `generation_requires_runtime`
  - `epoch_requires_runtime`
  - `binding_identity_present_when_runtime_bound`
- To: `Retired`

### `RecoverRuntimeAuthorityStopped`
- From: `Initializing`
- On: `RecoverRuntimeAuthority`(session_id, state, agent_runtime_id, fence_token, runtime_generation, runtime_epoch_id, current_run_id, pre_run_phase, silent_intent_overrides)
- Guards:
  - `observed_stopped`
  - `no_run_binding`
  - `fence_requires_runtime`
  - `generation_requires_runtime`
  - `epoch_requires_runtime`
  - `binding_identity_present_when_runtime_bound`
- To: `Stopped`

### `RecoverRuntimeAuthorityDestroyed`
- From: `Initializing`
- On: `RecoverRuntimeAuthority`(session_id, state, agent_runtime_id, fence_token, runtime_generation, runtime_epoch_id, current_run_id, pre_run_phase, silent_intent_overrides)
- Guards:
  - `observed_destroyed`
  - `no_run_binding`
  - `fence_requires_runtime`
  - `generation_requires_runtime`
  - `epoch_requires_runtime`
  - `binding_identity_present_when_runtime_bound`
- To: `Destroyed`

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
- On: `Ingest`(session_id, runtime_id, fence_token, generation, runtime_epoch_id, work_id, origin)
- Guards:
  - `session_matches_current`
  - `runtime_binding_matches_input`
  - `fence_binding_matches_input`
  - `generation_binding_matches_input`
  - `epoch_binding_matches_input`
- Emits: `ResolveAdmission`
- To: `Idle`

### `IngestAttached`
- From: `Attached`
- On: `Ingest`(session_id, runtime_id, fence_token, generation, runtime_epoch_id, work_id, origin)
- Guards:
  - `session_matches_current`
  - `runtime_binding_matches_input`
  - `fence_binding_matches_input`
  - `generation_binding_matches_input`
  - `epoch_binding_matches_input`
- Emits: `ResolveAdmission`
- To: `Attached`

### `IngestRunning`
- From: `Running`
- On: `Ingest`(session_id, runtime_id, fence_token, generation, runtime_epoch_id, work_id, origin)
- Guards:
  - `session_matches_current`
  - `runtime_binding_matches_input`
  - `fence_binding_matches_input`
  - `generation_binding_matches_input`
  - `epoch_binding_matches_input`
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
- Emits: `IngressAccepted`, `PostAdmissionSignal`
- To: `Running`

### `AcceptWithCompletionRunningImmediate`
- From: `Running`
- On: `AcceptWithCompletion`(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle)
- Guards:
  - `session_registered`
  - `request_immediate_processing`
  - `interrupt_yielding`
- Emits: `IngressAccepted`, `PostAdmissionSignal`
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

### `ResolveAdmissionValidationDurabilityRejectedIdle`
- From: `Idle`
- On: `ResolveAdmissionValidation`(input_id, input_kind, input_origin, durability, peer_handling_mode_valid, peer_response_terminal_structurally_valid, peer_response_terminal_observed_status)
- Guards:
  - `durability_invalid`
- Emits: `AdmissionValidationResolved`
- To: `Idle`

### `ResolveAdmissionValidationDurabilityRejectedAttached`
- From: `Attached`
- On: `ResolveAdmissionValidation`(input_id, input_kind, input_origin, durability, peer_handling_mode_valid, peer_response_terminal_structurally_valid, peer_response_terminal_observed_status)
- Guards:
  - `durability_invalid`
- Emits: `AdmissionValidationResolved`
- To: `Attached`

### `ResolveAdmissionValidationDurabilityRejectedRunning`
- From: `Running`
- On: `ResolveAdmissionValidation`(input_id, input_kind, input_origin, durability, peer_handling_mode_valid, peer_response_terminal_structurally_valid, peer_response_terminal_observed_status)
- Guards:
  - `durability_invalid`
- Emits: `AdmissionValidationResolved`
- To: `Running`

### `ResolveAdmissionValidationPeerHandlingRejectedIdle`
- From: `Idle`
- On: `ResolveAdmissionValidation`(input_id, input_kind, input_origin, durability, peer_handling_mode_valid, peer_response_terminal_structurally_valid, peer_response_terminal_observed_status)
- Guards:
  - `durability_authorized`
  - `peer_handling_mode_invalid`
- Emits: `AdmissionValidationResolved`
- To: `Idle`

### `ResolveAdmissionValidationPeerHandlingRejectedAttached`
- From: `Attached`
- On: `ResolveAdmissionValidation`(input_id, input_kind, input_origin, durability, peer_handling_mode_valid, peer_response_terminal_structurally_valid, peer_response_terminal_observed_status)
- Guards:
  - `durability_authorized`
  - `peer_handling_mode_invalid`
- Emits: `AdmissionValidationResolved`
- To: `Attached`

### `ResolveAdmissionValidationPeerHandlingRejectedRunning`
- From: `Running`
- On: `ResolveAdmissionValidation`(input_id, input_kind, input_origin, durability, peer_handling_mode_valid, peer_response_terminal_structurally_valid, peer_response_terminal_observed_status)
- Guards:
  - `durability_authorized`
  - `peer_handling_mode_invalid`
- Emits: `AdmissionValidationResolved`
- To: `Running`

### `ResolveAdmissionValidationPeerTerminalRejectedIdle`
- From: `Idle`
- On: `ResolveAdmissionValidation`(input_id, input_kind, input_origin, durability, peer_handling_mode_valid, peer_response_terminal_structurally_valid, peer_response_terminal_observed_status)
- Guards:
  - `durability_authorized`
  - `peer_handling_mode_valid`
  - `peer_response_terminal_invalid`
- Emits: `AdmissionValidationResolved`
- To: `Idle`

### `ResolveAdmissionValidationPeerTerminalRejectedAttached`
- From: `Attached`
- On: `ResolveAdmissionValidation`(input_id, input_kind, input_origin, durability, peer_handling_mode_valid, peer_response_terminal_structurally_valid, peer_response_terminal_observed_status)
- Guards:
  - `durability_authorized`
  - `peer_handling_mode_valid`
  - `peer_response_terminal_invalid`
- Emits: `AdmissionValidationResolved`
- To: `Attached`

### `ResolveAdmissionValidationPeerTerminalRejectedRunning`
- From: `Running`
- On: `ResolveAdmissionValidation`(input_id, input_kind, input_origin, durability, peer_handling_mode_valid, peer_response_terminal_structurally_valid, peer_response_terminal_observed_status)
- Guards:
  - `durability_authorized`
  - `peer_handling_mode_valid`
  - `peer_response_terminal_invalid`
- Emits: `AdmissionValidationResolved`
- To: `Running`

### `ResolveAdmissionValidationAcceptedIdle`
- From: `Idle`
- On: `ResolveAdmissionValidation`(input_id, input_kind, input_origin, durability, peer_handling_mode_valid, peer_response_terminal_structurally_valid, peer_response_terminal_observed_status)
- Guards:
  - `durability_authorized`
  - `peer_handling_mode_valid`
  - `peer_response_terminal_structurally_valid`
  - `peer_response_terminal_status_supported`
- Emits: `AdmissionValidationResolved`
- To: `Idle`

### `ResolveAdmissionValidationAcceptedAttached`
- From: `Attached`
- On: `ResolveAdmissionValidation`(input_id, input_kind, input_origin, durability, peer_handling_mode_valid, peer_response_terminal_structurally_valid, peer_response_terminal_observed_status)
- Guards:
  - `durability_authorized`
  - `peer_handling_mode_valid`
  - `peer_response_terminal_structurally_valid`
  - `peer_response_terminal_status_supported`
- Emits: `AdmissionValidationResolved`
- To: `Attached`

### `ResolveAdmissionValidationAcceptedRunning`
- From: `Running`
- On: `ResolveAdmissionValidation`(input_id, input_kind, input_origin, durability, peer_handling_mode_valid, peer_response_terminal_structurally_valid, peer_response_terminal_observed_status)
- Guards:
  - `durability_authorized`
  - `peer_handling_mode_valid`
  - `peer_response_terminal_structurally_valid`
  - `peer_response_terminal_status_supported`
- Emits: `AdmissionValidationResolved`
- To: `Running`

### `NormalizeRecoveredInputAcceptedQueueInitializing`
- From: `Initializing`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `accepted_phase`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Initializing`

### `NormalizeRecoveredInputAcceptedQueueIdle`
- From: `Idle`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `accepted_phase`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Idle`

### `NormalizeRecoveredInputAcceptedQueueAttached`
- From: `Attached`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `accepted_phase`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Attached`

### `NormalizeRecoveredInputAcceptedQueueRunning`
- From: `Running`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `accepted_phase`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Running`

### `NormalizeRecoveredInputAcceptedQueueRetired`
- From: `Retired`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `accepted_phase`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Retired`

### `NormalizeRecoveredInputAcceptedQueueStopped`
- From: `Stopped`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `accepted_phase`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Stopped`

### `NormalizeRecoveredInputStagedInitializing`
- From: `Initializing`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `staged_phase`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Initializing`

### `NormalizeRecoveredInputStagedIdle`
- From: `Idle`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `staged_phase`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Idle`

### `NormalizeRecoveredInputStagedAttached`
- From: `Attached`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `staged_phase`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Attached`

### `NormalizeRecoveredInputStagedRunning`
- From: `Running`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `staged_phase`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Running`

### `NormalizeRecoveredInputStagedRetired`
- From: `Retired`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `staged_phase`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Retired`

### `NormalizeRecoveredInputStagedStopped`
- From: `Stopped`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `staged_phase`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Stopped`

### `NormalizeRecoveredInputAppliedCommittedInitializing`
- From: `Initializing`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `applied_phase`
  - `boundary_committed_observed`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Initializing`

### `NormalizeRecoveredInputAppliedCommittedIdle`
- From: `Idle`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `applied_phase`
  - `boundary_committed_observed`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Idle`

### `NormalizeRecoveredInputAppliedCommittedAttached`
- From: `Attached`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `applied_phase`
  - `boundary_committed_observed`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Attached`

### `NormalizeRecoveredInputAppliedCommittedRunning`
- From: `Running`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `applied_phase`
  - `boundary_committed_observed`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Running`

### `NormalizeRecoveredInputAppliedCommittedRetired`
- From: `Retired`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `applied_phase`
  - `boundary_committed_observed`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Retired`

### `NormalizeRecoveredInputAppliedCommittedStopped`
- From: `Stopped`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `applied_phase`
  - `boundary_committed_observed`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Stopped`

### `NormalizeRecoveredInputAppliedMissingReceiptInitializing`
- From: `Initializing`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `applied_phase`
  - `boundary_missing_observed`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Initializing`

### `NormalizeRecoveredInputAppliedMissingReceiptIdle`
- From: `Idle`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `applied_phase`
  - `boundary_missing_observed`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Idle`

### `NormalizeRecoveredInputAppliedMissingReceiptAttached`
- From: `Attached`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `applied_phase`
  - `boundary_missing_observed`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Attached`

### `NormalizeRecoveredInputAppliedMissingReceiptRunning`
- From: `Running`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `applied_phase`
  - `boundary_missing_observed`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Running`

### `NormalizeRecoveredInputAppliedMissingReceiptRetired`
- From: `Retired`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `applied_phase`
  - `boundary_missing_observed`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Retired`

### `NormalizeRecoveredInputAppliedMissingReceiptStopped`
- From: `Stopped`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `applied_phase`
  - `boundary_missing_observed`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Stopped`

### `NormalizeRecoveredInputAppliedUnobservedReceiptInitializing`
- From: `Initializing`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `applied_phase`
  - `boundary_receipt_unobserved`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Initializing`

### `NormalizeRecoveredInputAppliedUnobservedReceiptIdle`
- From: `Idle`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `applied_phase`
  - `boundary_receipt_unobserved`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Idle`

### `NormalizeRecoveredInputAppliedUnobservedReceiptAttached`
- From: `Attached`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `applied_phase`
  - `boundary_receipt_unobserved`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Attached`

### `NormalizeRecoveredInputAppliedUnobservedReceiptRunning`
- From: `Running`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `applied_phase`
  - `boundary_receipt_unobserved`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Running`

### `NormalizeRecoveredInputAppliedUnobservedReceiptRetired`
- From: `Retired`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `applied_phase`
  - `boundary_receipt_unobserved`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Retired`

### `NormalizeRecoveredInputAppliedUnobservedReceiptStopped`
- From: `Stopped`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `applied_phase`
  - `boundary_receipt_unobserved`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Stopped`

### `NormalizeRecoveredInputQueuedInitializing`
- From: `Initializing`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `queued_phase`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Initializing`

### `NormalizeRecoveredInputQueuedIdle`
- From: `Idle`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `queued_phase`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Idle`

### `NormalizeRecoveredInputQueuedAttached`
- From: `Attached`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `queued_phase`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Attached`

### `NormalizeRecoveredInputQueuedRunning`
- From: `Running`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `queued_phase`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Running`

### `NormalizeRecoveredInputQueuedRetired`
- From: `Retired`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `queued_phase`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Retired`

### `NormalizeRecoveredInputQueuedStopped`
- From: `Stopped`
- On: `NormalizeRecoveredInputLifecycle`(input_id, phase, applied_boundary_committed)
- Guards:
  - `queued_phase`
- Emits: `RecoveredInputLifecycleNormalized`
- To: `Stopped`

### `ClassifyRecoveredInputDurabilityDiscardEphemeralInitializing`
- From: `Initializing`
- On: `ClassifyRecoveredInputDurability`(input_id, durability)
- Guards:
  - `ephemeral_durability`
- Emits: `RecoveredInputDurabilityClassified`
- To: `Initializing`

### `ClassifyRecoveredInputDurabilityDiscardEphemeralIdle`
- From: `Idle`
- On: `ClassifyRecoveredInputDurability`(input_id, durability)
- Guards:
  - `ephemeral_durability`
- Emits: `RecoveredInputDurabilityClassified`
- To: `Idle`

### `ClassifyRecoveredInputDurabilityDiscardEphemeralAttached`
- From: `Attached`
- On: `ClassifyRecoveredInputDurability`(input_id, durability)
- Guards:
  - `ephemeral_durability`
- Emits: `RecoveredInputDurabilityClassified`
- To: `Attached`

### `ClassifyRecoveredInputDurabilityDiscardEphemeralRunning`
- From: `Running`
- On: `ClassifyRecoveredInputDurability`(input_id, durability)
- Guards:
  - `ephemeral_durability`
- Emits: `RecoveredInputDurabilityClassified`
- To: `Running`

### `ClassifyRecoveredInputDurabilityDiscardEphemeralRetired`
- From: `Retired`
- On: `ClassifyRecoveredInputDurability`(input_id, durability)
- Guards:
  - `ephemeral_durability`
- Emits: `RecoveredInputDurabilityClassified`
- To: `Retired`

### `ClassifyRecoveredInputDurabilityDiscardEphemeralStopped`
- From: `Stopped`
- On: `ClassifyRecoveredInputDurability`(input_id, durability)
- Guards:
  - `ephemeral_durability`
- Emits: `RecoveredInputDurabilityClassified`
- To: `Stopped`

### `ClassifyRecoveredInputDurabilityRetainDurableDerivedOrMissingInitializing`
- From: `Initializing`
- On: `ClassifyRecoveredInputDurability`(input_id, durability)
- Guards:
  - `retained_durability`
- Emits: `RecoveredInputDurabilityClassified`
- To: `Initializing`

### `ClassifyRecoveredInputDurabilityRetainDurableDerivedOrMissingIdle`
- From: `Idle`
- On: `ClassifyRecoveredInputDurability`(input_id, durability)
- Guards:
  - `retained_durability`
- Emits: `RecoveredInputDurabilityClassified`
- To: `Idle`

### `ClassifyRecoveredInputDurabilityRetainDurableDerivedOrMissingAttached`
- From: `Attached`
- On: `ClassifyRecoveredInputDurability`(input_id, durability)
- Guards:
  - `retained_durability`
- Emits: `RecoveredInputDurabilityClassified`
- To: `Attached`

### `ClassifyRecoveredInputDurabilityRetainDurableDerivedOrMissingRunning`
- From: `Running`
- On: `ClassifyRecoveredInputDurability`(input_id, durability)
- Guards:
  - `retained_durability`
- Emits: `RecoveredInputDurabilityClassified`
- To: `Running`

### `ClassifyRecoveredInputDurabilityRetainDurableDerivedOrMissingRetired`
- From: `Retired`
- On: `ClassifyRecoveredInputDurability`(input_id, durability)
- Guards:
  - `retained_durability`
- Emits: `RecoveredInputDurabilityClassified`
- To: `Retired`

### `ClassifyRecoveredInputDurabilityRetainDurableDerivedOrMissingStopped`
- From: `Stopped`
- On: `ClassifyRecoveredInputDurability`(input_id, durability)
- Guards:
  - `retained_durability`
- Emits: `RecoveredInputDurabilityClassified`
- To: `Stopped`

### `ResolveInputPublicLifecycleAcceptedIdle`
- From: `Idle`
- On: `ResolveInputPublicLifecycle`(input_id, phase)
- Guards:
  - `accepted_phase`
- Emits: `InputPublicLifecycleResolved`
- To: `Idle`

### `ResolveInputPublicLifecycleQueuedIdle`
- From: `Idle`
- On: `ResolveInputPublicLifecycle`(input_id, phase)
- Guards:
  - `queued_phase`
- Emits: `InputPublicLifecycleResolved`
- To: `Idle`

### `ResolveInputPublicLifecycleStagedIdle`
- From: `Idle`
- On: `ResolveInputPublicLifecycle`(input_id, phase)
- Guards:
  - `staged_phase`
- Emits: `InputPublicLifecycleResolved`
- To: `Idle`

### `ResolveInputPublicLifecycleAppliedIdle`
- From: `Idle`
- On: `ResolveInputPublicLifecycle`(input_id, phase)
- Guards:
  - `applied_phase`
- Emits: `InputPublicLifecycleResolved`
- To: `Idle`

### `ResolveInputPublicLifecycleAppliedPendingConsumptionIdle`
- From: `Idle`
- On: `ResolveInputPublicLifecycle`(input_id, phase)
- Guards:
  - `applied_pending_consumption_phase`
- Emits: `InputPublicLifecycleResolved`
- To: `Idle`

### `ResolveInputPublicLifecycleConsumedIdle`
- From: `Idle`
- On: `ResolveInputPublicLifecycle`(input_id, phase)
- Guards:
  - `consumed_phase`
- Emits: `InputPublicLifecycleResolved`
- To: `Idle`

### `ResolveInputPublicLifecycleSupersededIdle`
- From: `Idle`
- On: `ResolveInputPublicLifecycle`(input_id, phase)
- Guards:
  - `superseded_phase`
- Emits: `InputPublicLifecycleResolved`
- To: `Idle`

### `ResolveInputPublicLifecycleCoalescedIdle`
- From: `Idle`
- On: `ResolveInputPublicLifecycle`(input_id, phase)
- Guards:
  - `coalesced_phase`
- Emits: `InputPublicLifecycleResolved`
- To: `Idle`

### `ResolveInputPublicLifecycleAbandonedIdle`
- From: `Idle`
- On: `ResolveInputPublicLifecycle`(input_id, phase)
- Guards:
  - `abandoned_phase`
- Emits: `InputPublicLifecycleResolved`
- To: `Idle`

### `ResolveInputPublicTerminalOutcomeNonTerminalIdle`
- From: `Idle`
- On: `ResolveInputPublicTerminalOutcome`(input_id, phase, terminal_kind, abandon_reason)
- Guards:
  - `non_terminal_phase`
  - `terminal_absent`
- Emits: `InputPublicTerminalOutcomeResolved`
- To: `Idle`

### `ResolveInputPublicTerminalOutcomeConsumedIdle`
- From: `Idle`
- On: `ResolveInputPublicTerminalOutcome`(input_id, phase, terminal_kind, abandon_reason)
- Guards:
  - `consumed_phase`
  - `consumed_terminal`
- Emits: `InputPublicTerminalOutcomeResolved`
- To: `Idle`

### `ResolveInputPublicTerminalOutcomeSupersededIdle`
- From: `Idle`
- On: `ResolveInputPublicTerminalOutcome`(input_id, phase, terminal_kind, abandon_reason)
- Guards:
  - `superseded_phase`
  - `superseded_terminal`
- Emits: `InputPublicTerminalOutcomeResolved`
- To: `Idle`

### `ResolveInputPublicTerminalOutcomeCoalescedIdle`
- From: `Idle`
- On: `ResolveInputPublicTerminalOutcome`(input_id, phase, terminal_kind, abandon_reason)
- Guards:
  - `coalesced_phase`
  - `coalesced_terminal`
- Emits: `InputPublicTerminalOutcomeResolved`
- To: `Idle`

### `ResolveInputPublicTerminalOutcomeCancelledIdle`
- From: `Idle`
- On: `ResolveInputPublicTerminalOutcome`(input_id, phase, terminal_kind, abandon_reason)
- Guards:
  - `abandoned_phase`
  - `cancelled_terminal`
- Emits: `InputPublicTerminalOutcomeResolved`
- To: `Idle`

### `ResolveInputPublicTerminalOutcomeAbandonedIdle`
- From: `Idle`
- On: `ResolveInputPublicTerminalOutcome`(input_id, phase, terminal_kind, abandon_reason)
- Guards:
  - `abandoned_phase`
  - `abandoned_terminal`
- Emits: `InputPublicTerminalOutcomeResolved`
- To: `Idle`

### `ClassifyInputTerminalityNonTerminalIdle`
- From: `Idle`
- On: `ClassifyInputTerminality`(input_id, phase, terminal_kind, abandon_reason)
- Guards:
  - `non_terminal_phase`
  - `terminal_absent`
- Emits: `InputBehavioralTerminalityResolved`
- To: `Idle`

### `ClassifyInputTerminalityConsumedIdle`
- From: `Idle`
- On: `ClassifyInputTerminality`(input_id, phase, terminal_kind, abandon_reason)
- Guards:
  - `consumed_phase`
  - `consumed_terminal`
- Emits: `InputBehavioralTerminalityResolved`
- To: `Idle`

### `ClassifyInputTerminalitySupersededIdle`
- From: `Idle`
- On: `ClassifyInputTerminality`(input_id, phase, terminal_kind, abandon_reason)
- Guards:
  - `superseded_phase`
  - `superseded_terminal`
- Emits: `InputBehavioralTerminalityResolved`
- To: `Idle`

### `ClassifyInputTerminalityCoalescedIdle`
- From: `Idle`
- On: `ClassifyInputTerminality`(input_id, phase, terminal_kind, abandon_reason)
- Guards:
  - `coalesced_phase`
  - `coalesced_terminal`
- Emits: `InputBehavioralTerminalityResolved`
- To: `Idle`

### `ClassifyInputTerminalityAbandonedIdle`
- From: `Idle`
- On: `ClassifyInputTerminality`(input_id, phase, terminal_kind, abandon_reason)
- Guards:
  - `abandoned_phase`
  - `abandoned_terminal`
- Emits: `InputBehavioralTerminalityResolved`
- To: `Idle`

### `AuthorizeStoredInputStateSeedIdle`
- From: `Idle`
- On: `AuthorizeStoredInputStateSeed`(input_id, phase, terminal_kind, superseded_by, aggregate_id, abandon_reason, abandon_attempt_count, attempt_count, run_id, boundary_sequence, admission_sequence, recovery_lane)
- Guards:
  - `stored_seed_terminal_payload_matches_phase`
  - `stored_seed_terminal_has_no_recovery_lane`
  - `stored_seed_max_attempts_reason_matches_count`
  - `stored_seed_max_attempts_reason_matches_policy`
- Emits: `StoredInputStateSeedAuthorized`
- To: `Idle`

### `ClassifyRuntimeLifecycleInitializingIdle`
- From: `Idle`
- On: `ClassifyRuntimeLifecycleState`(state)
- Guards:
  - `initializing_state`
- Emits: `RuntimeLifecycleStateClassified`
- To: `Idle`

### `ClassifyRuntimeLifecycleIdleIdle`
- From: `Idle`
- On: `ClassifyRuntimeLifecycleState`(state)
- Guards:
  - `idle_state`
- Emits: `RuntimeLifecycleStateClassified`
- To: `Idle`

### `ClassifyRuntimeLifecycleAttachedIdle`
- From: `Idle`
- On: `ClassifyRuntimeLifecycleState`(state)
- Guards:
  - `attached_state`
- Emits: `RuntimeLifecycleStateClassified`
- To: `Idle`

### `ClassifyRuntimeLifecycleRunningIdle`
- From: `Idle`
- On: `ClassifyRuntimeLifecycleState`(state)
- Guards:
  - `running_state`
- Emits: `RuntimeLifecycleStateClassified`
- To: `Idle`

### `ClassifyRuntimeLifecycleRetiredIdle`
- From: `Idle`
- On: `ClassifyRuntimeLifecycleState`(state)
- Guards:
  - `retired_state`
- Emits: `RuntimeLifecycleStateClassified`
- To: `Idle`

### `ClassifyRuntimeLifecycleStoppedIdle`
- From: `Idle`
- On: `ClassifyRuntimeLifecycleState`(state)
- Guards:
  - `stopped_state`
- Emits: `RuntimeLifecycleStateClassified`
- To: `Idle`

### `ClassifyRuntimeLifecycleDestroyedIdle`
- From: `Idle`
- On: `ClassifyRuntimeLifecycleState`(state)
- Guards:
  - `destroyed_state`
- Emits: `RuntimeLifecycleStateClassified`
- To: `Idle`

### `ClassifyRuntimeDurabilityInitializingIdle`
- From: `Idle`
- On: `ClassifyRuntimeLifecycleDurability`(state)
- Guards:
  - `initializing_state`
- Emits: `RuntimeLifecycleDurabilityClassified`
- To: `Idle`

### `ClassifyRuntimeDurabilityIdleIdle`
- From: `Idle`
- On: `ClassifyRuntimeLifecycleDurability`(state)
- Guards:
  - `idle_state`
- Emits: `RuntimeLifecycleDurabilityClassified`
- To: `Idle`

### `ClassifyRuntimeDurabilityAttachedIdle`
- From: `Idle`
- On: `ClassifyRuntimeLifecycleDurability`(state)
- Guards:
  - `attached_state`
- Emits: `RuntimeLifecycleDurabilityClassified`
- To: `Idle`

### `ClassifyRuntimeDurabilityRunningIdle`
- From: `Idle`
- On: `ClassifyRuntimeLifecycleDurability`(state)
- Guards:
  - `running_state`
- Emits: `RuntimeLifecycleDurabilityClassified`
- To: `Idle`

### `ClassifyRuntimeDurabilityRetiredIdle`
- From: `Idle`
- On: `ClassifyRuntimeLifecycleDurability`(state)
- Guards:
  - `retired_state`
- Emits: `RuntimeLifecycleDurabilityClassified`
- To: `Idle`

### `ClassifyRuntimeDurabilityStoppedIdle`
- From: `Idle`
- On: `ClassifyRuntimeLifecycleDurability`(state)
- Guards:
  - `stopped_state`
- Emits: `RuntimeLifecycleDurabilityClassified`
- To: `Idle`

### `ClassifyRuntimeDurabilityDestroyedIdle`
- From: `Idle`
- On: `ClassifyRuntimeLifecycleDurability`(state)
- Guards:
  - `destroyed_state`
- Emits: `RuntimeLifecycleDurabilityClassified`
- To: `Idle`

### `ClassifyRuntimeLoopQueueInitializingIdle`
- From: `Idle`
- On: `ClassifyRuntimeLoopQueueAdmission`(state, current_run_bound)
- Guards:
  - `initializing_state`
  - `no_current_run`
- Emits: `RuntimeLoopQueueAdmissionClassified`
- To: `Idle`

### `ClassifyRuntimeLoopQueueIdleIdle`
- From: `Idle`
- On: `ClassifyRuntimeLoopQueueAdmission`(state, current_run_bound)
- Guards:
  - `idle_state`
  - `no_current_run`
- Emits: `RuntimeLoopQueueAdmissionClassified`
- To: `Idle`

### `ClassifyRuntimeLoopQueueAttachedIdle`
- From: `Idle`
- On: `ClassifyRuntimeLoopQueueAdmission`(state, current_run_bound)
- Guards:
  - `attached_state`
  - `no_current_run`
- Emits: `RuntimeLoopQueueAdmissionClassified`
- To: `Idle`

### `ClassifyRuntimeLoopQueueRunningWithoutBindingIdle`
- From: `Idle`
- On: `ClassifyRuntimeLoopQueueAdmission`(state, current_run_bound)
- Guards:
  - `running_state`
  - `no_current_run`
- Emits: `RuntimeLoopQueueAdmissionClassified`
- To: `Idle`

### `ClassifyRuntimeLoopQueueRunningWithBindingIdle`
- From: `Idle`
- On: `ClassifyRuntimeLoopQueueAdmission`(state, current_run_bound)
- Guards:
  - `running_state`
  - `has_current_run`
- Emits: `RuntimeLoopQueueAdmissionClassified`
- To: `Idle`

### `ClassifyRuntimeLoopQueueRetiredIdle`
- From: `Idle`
- On: `ClassifyRuntimeLoopQueueAdmission`(state, current_run_bound)
- Guards:
  - `retired_state`
  - `no_current_run`
- Emits: `RuntimeLoopQueueAdmissionClassified`
- To: `Idle`

### `ClassifyRuntimeLoopQueueStoppedIdle`
- From: `Idle`
- On: `ClassifyRuntimeLoopQueueAdmission`(state, current_run_bound)
- Guards:
  - `stopped_state`
  - `no_current_run`
- Emits: `RuntimeLoopQueueAdmissionClassified`
- To: `Idle`

### `ClassifyRuntimeLoopQueueDestroyedIdle`
- From: `Idle`
- On: `ClassifyRuntimeLoopQueueAdmission`(state, current_run_bound)
- Guards:
  - `destroyed_state`
  - `no_current_run`
- Emits: `RuntimeLoopQueueAdmissionClassified`
- To: `Idle`

### `ResolveVisibleRuntimePhasePublishControlVisibleRewriteIdle`
- From: `Idle`
- On: `ResolveVisibleRuntimePhase`(dsl_phase, dsl_pre_run_phase, control_phase, control_pre_run_phase, has_runtime_persistence)
- Guards:
  - `publish_control`
  - `control_visible_rewrite`
- Emits: `VisibleRuntimePhaseResolved`
- To: `Idle`

### `ResolveVisibleRuntimePhasePublishControlNoRewriteIdle`
- From: `Idle`
- On: `ResolveVisibleRuntimePhase`(dsl_phase, dsl_pre_run_phase, control_phase, control_pre_run_phase, has_runtime_persistence)
- Guards:
  - `publish_control`
  - `control_no_visible_rewrite`
- Emits: `VisibleRuntimePhaseResolved`
- To: `Idle`

### `ResolveVisibleRuntimePhaseKeepDslVisibleRewriteIdle`
- From: `Idle`
- On: `ResolveVisibleRuntimePhase`(dsl_phase, dsl_pre_run_phase, control_phase, control_pre_run_phase, has_runtime_persistence)
- Guards:
  - `keep_dsl`
  - `dsl_visible_rewrite`
- Emits: `VisibleRuntimePhaseResolved`
- To: `Idle`

### `ResolveVisibleRuntimePhaseKeepDslNoRewriteIdle`
- From: `Idle`
- On: `ResolveVisibleRuntimePhase`(dsl_phase, dsl_pre_run_phase, control_phase, control_pre_run_phase, has_runtime_persistence)
- Guards:
  - `keep_dsl`
  - `dsl_no_visible_rewrite`
- Emits: `VisibleRuntimePhaseResolved`
- To: `Idle`

### `ResolveAdmissionIdempotencyNoKeyIdle`
- From: `Idle`
- On: `ResolveAdmissionIdempotency`(input_id, idempotency_key)
- Guards:
  - `no_idempotency_key`
- Emits: `AdmissionIdempotencyResolved`
- To: `Idle`

### `ResolveAdmissionIdempotencyNoKeyAttached`
- From: `Attached`
- On: `ResolveAdmissionIdempotency`(input_id, idempotency_key)
- Guards:
  - `no_idempotency_key`
- Emits: `AdmissionIdempotencyResolved`
- To: `Attached`

### `ResolveAdmissionIdempotencyNoKeyRunning`
- From: `Running`
- On: `ResolveAdmissionIdempotency`(input_id, idempotency_key)
- Guards:
  - `no_idempotency_key`
- Emits: `AdmissionIdempotencyResolved`
- To: `Running`

### `ResolveAdmissionIdempotencyNewKeyIdle`
- From: `Idle`
- On: `ResolveAdmissionIdempotency`(input_id, idempotency_key)
- Guards:
  - `idempotency_key_present`
  - `idempotency_key_unclaimed`
- Emits: `AdmissionIdempotencyResolved`
- To: `Idle`

### `ResolveAdmissionIdempotencyNewKeyAttached`
- From: `Attached`
- On: `ResolveAdmissionIdempotency`(input_id, idempotency_key)
- Guards:
  - `idempotency_key_present`
  - `idempotency_key_unclaimed`
- Emits: `AdmissionIdempotencyResolved`
- To: `Attached`

### `ResolveAdmissionIdempotencyNewKeyRunning`
- From: `Running`
- On: `ResolveAdmissionIdempotency`(input_id, idempotency_key)
- Guards:
  - `idempotency_key_present`
  - `idempotency_key_unclaimed`
- Emits: `AdmissionIdempotencyResolved`
- To: `Running`

### `ResolveAdmissionIdempotencyDuplicateIdle`
- From: `Idle`
- On: `ResolveAdmissionIdempotency`(input_id, idempotency_key)
- Guards:
  - `idempotency_key_present`
  - `idempotency_key_claimed`
- Emits: `AdmissionIdempotencyResolved`
- To: `Idle`

### `ResolveAdmissionIdempotencyDuplicateAttached`
- From: `Attached`
- On: `ResolveAdmissionIdempotency`(input_id, idempotency_key)
- Guards:
  - `idempotency_key_present`
  - `idempotency_key_claimed`
- Emits: `AdmissionIdempotencyResolved`
- To: `Attached`

### `ResolveAdmissionIdempotencyDuplicateRunning`
- From: `Running`
- On: `ResolveAdmissionIdempotency`(input_id, idempotency_key)
- Guards:
  - `idempotency_key_present`
  - `idempotency_key_claimed`
- Emits: `AdmissionIdempotencyResolved`
- To: `Running`

### `RegisterAcceptedIdempotencyIdle`
- From: `Idle`
- On: `RegisterAcceptedIdempotency`(input_id, idempotency_key)
- Guards:
  - `input_tracked`
  - `idempotency_key_unclaimed_or_same_input`
- Emits: `InputLifecycleNotice`
- To: `Idle`

### `RegisterAcceptedIdempotencyAttached`
- From: `Attached`
- On: `RegisterAcceptedIdempotency`(input_id, idempotency_key)
- Guards:
  - `input_tracked`
  - `idempotency_key_unclaimed_or_same_input`
- Emits: `InputLifecycleNotice`
- To: `Attached`

### `RegisterAcceptedIdempotencyRunning`
- From: `Running`
- On: `RegisterAcceptedIdempotency`(input_id, idempotency_key)
- Guards:
  - `input_tracked`
  - `idempotency_key_unclaimed_or_same_input`
- Emits: `InputLifecycleNotice`
- To: `Running`

### `RegisterAcceptedIdempotencyRetired`
- From: `Retired`
- On: `RegisterAcceptedIdempotency`(input_id, idempotency_key)
- Guards:
  - `input_tracked`
  - `idempotency_key_unclaimed_or_same_input`
- Emits: `InputLifecycleNotice`
- To: `Retired`

### `RegisterAcceptedIdempotencyStopped`
- From: `Stopped`
- On: `RegisterAcceptedIdempotency`(input_id, idempotency_key)
- Guards:
  - `input_tracked`
  - `idempotency_key_unclaimed_or_same_input`
- Emits: `InputLifecycleNotice`
- To: `Stopped`

### `ResolveAdmissionPlanRequestedTerminalQueueIdle`
- From: `Idle`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `terminal_queue_override`
- Emits: `AdmissionResolved`
- To: `Idle`

### `ResolveAdmissionPlanRequestedTerminalQueueAttached`
- From: `Attached`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `terminal_queue_override`
- Emits: `AdmissionResolved`
- To: `Attached`

### `ResolveAdmissionPlanRequestedTerminalQueueRunning`
- From: `Running`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `terminal_queue_override`
- Emits: `AdmissionResolved`
- To: `Running`

### `ResolveAdmissionPlanRequestedTerminalSteerIdle`
- From: `Idle`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `terminal_steer_override`
- Emits: `AdmissionResolved`
- To: `Idle`

### `ResolveAdmissionPlanRequestedTerminalSteerAttached`
- From: `Attached`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `terminal_steer_override`
- Emits: `AdmissionResolved`
- To: `Attached`

### `ResolveAdmissionPlanRequestedTerminalSteerRunning`
- From: `Running`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `terminal_steer_override`
- Emits: `AdmissionResolved`
- To: `Running`

### `ResolveAdmissionPlanRequestedQueueIdle`
- From: `Idle`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `queue_override`
- Emits: `AdmissionResolved`
- To: `Idle`

### `ResolveAdmissionPlanRequestedQueueAttached`
- From: `Attached`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `queue_override`
- Emits: `AdmissionResolved`
- To: `Attached`

### `ResolveAdmissionPlanRequestedQueueRunning`
- From: `Running`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `queue_override`
- Emits: `AdmissionResolved`
- To: `Running`

### `ResolveAdmissionPlanRequestedSteerIdle`
- From: `Idle`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `steer_override`
- Emits: `AdmissionResolved`
- To: `Idle`

### `ResolveAdmissionPlanRequestedSteerAttached`
- From: `Attached`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `steer_override`
- Emits: `AdmissionResolved`
- To: `Attached`

### `ResolveAdmissionPlanRequestedSteerRunning`
- From: `Running`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `steer_override`
- Emits: `AdmissionResolved`
- To: `Running`

### `ResolveAdmissionPlanDefaultQueueKindIdle`
- From: `Idle`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `default_queue_kind`
- Emits: `AdmissionResolved`
- To: `Idle`

### `ResolveAdmissionPlanDefaultQueueKindAttached`
- From: `Attached`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `default_queue_kind`
- Emits: `AdmissionResolved`
- To: `Attached`

### `ResolveAdmissionPlanDefaultQueueKindRunning`
- From: `Running`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `default_queue_kind`
- Emits: `AdmissionResolved`
- To: `Running`

### `ResolveAdmissionPlanDefaultPeerMessageOrRequestIdle`
- From: `Idle`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `default_peer_message_or_request`
- Emits: `AdmissionResolved`
- To: `Idle`

### `ResolveAdmissionPlanDefaultPeerMessageOrRequestAttached`
- From: `Attached`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `default_peer_message_or_request`
- Emits: `AdmissionResolved`
- To: `Attached`

### `ResolveAdmissionPlanDefaultPeerMessageOrRequestRunning`
- From: `Running`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `default_peer_message_or_request`
- Emits: `AdmissionResolved`
- To: `Running`

### `ResolveAdmissionPlanPeerResponseProgressIdle`
- From: `Idle`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `peer_response_progress`
  - `coalesce_target_tracked_if_present`
- Emits: `AdmissionResolved`
- To: `Idle`

### `ResolveAdmissionPlanPeerResponseProgressAttached`
- From: `Attached`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `peer_response_progress`
  - `coalesce_target_tracked_if_present`
- Emits: `AdmissionResolved`
- To: `Attached`

### `ResolveAdmissionPlanPeerResponseProgressRunning`
- From: `Running`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `peer_response_progress`
  - `coalesce_target_tracked_if_present`
- Emits: `AdmissionResolved`
- To: `Running`

### `ResolveAdmissionPlanDefaultPeerResponseTerminalIdle`
- From: `Idle`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `default_peer_response_terminal`
- Emits: `AdmissionResolved`
- To: `Idle`

### `ResolveAdmissionPlanDefaultPeerResponseTerminalAttached`
- From: `Attached`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `default_peer_response_terminal`
- Emits: `AdmissionResolved`
- To: `Attached`

### `ResolveAdmissionPlanDefaultPeerResponseTerminalRunning`
- From: `Running`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `default_peer_response_terminal`
- Emits: `AdmissionResolved`
- To: `Running`

### `ResolveAdmissionPlanDefaultContinuationIdle`
- From: `Idle`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `default_continuation`
- Emits: `AdmissionResolved`
- To: `Idle`

### `ResolveAdmissionPlanDefaultContinuationAttached`
- From: `Attached`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `default_continuation`
- Emits: `AdmissionResolved`
- To: `Attached`

### `ResolveAdmissionPlanDefaultContinuationRunning`
- From: `Running`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `default_continuation`
- Emits: `AdmissionResolved`
- To: `Running`

### `ResolveAdmissionPlanWorkgraphAttentionContinuationIdle`
- From: `Idle`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `workgraph_attention_continuation`
- Emits: `AdmissionResolved`
- To: `Idle`

### `ResolveAdmissionPlanWorkgraphAttentionContinuationAttached`
- From: `Attached`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `workgraph_attention_continuation`
- Emits: `AdmissionResolved`
- To: `Attached`

### `ResolveAdmissionPlanWorkgraphAttentionContinuationRunning`
- From: `Running`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `workgraph_attention_continuation`
- Emits: `AdmissionResolved`
- To: `Running`

### `ResolveAdmissionPlanOperationIdle`
- From: `Idle`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `operation`
- Emits: `AdmissionResolved`
- To: `Idle`

### `ResolveAdmissionPlanOperationAttached`
- From: `Attached`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `operation`
- Emits: `AdmissionResolved`
- To: `Attached`

### `ResolveAdmissionPlanOperationRunning`
- From: `Running`
- On: `ResolveAdmissionPlan`(input_id, input_kind, requested_lane, continuation_kind, silent_intent_match, existing_superseded_input_id, runtime_running, active_turn_boundary_available, without_wake)
- Guards:
  - `runtime_running_matches_phase`
  - `operation`
- Emits: `AdmissionResolved`
- To: `Running`

### `ClassifyExternalEnvelopeMessageAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_message`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeMessageRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_message`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeRequestPeerAddedAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_request_peer_added`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeRequestPeerAddedIdle`
- From: `Idle`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_request_peer_added`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Idle`

### `ClassifyExternalEnvelopeRequestPeerAddedRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_request_peer_added`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeRequestPeerRetiredAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_request_peer_retired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeRequestPeerRetiredIdle`
- From: `Idle`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_request_peer_retired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Idle`

### `ClassifyExternalEnvelopeRequestPeerRetiredRetired`
- From: `Retired`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_request_peer_retired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Retired`

### `ClassifyExternalEnvelopeRequestPeerRetiredStopped`
- From: `Stopped`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_request_peer_retired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Stopped`

### `ClassifyExternalEnvelopeRequestPeerRetiredRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_request_peer_retired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeRequestPeerUnwiredAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_request_peer_unwired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeRequestPeerUnwiredIdle`
- From: `Idle`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_request_peer_unwired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Idle`

### `ClassifyExternalEnvelopeRequestPeerUnwiredRetired`
- From: `Retired`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_request_peer_unwired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Retired`

### `ClassifyExternalEnvelopeRequestPeerUnwiredStopped`
- From: `Stopped`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_request_peer_unwired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Stopped`

### `ClassifyExternalEnvelopeRequestPeerUnwiredRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_request_peer_unwired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeRequestSupervisorSilentAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_supervisor_silent_request`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeRequestSupervisorSilentIdle`
- From: `Idle`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_supervisor_silent_request`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Idle`

### `ClassifyExternalEnvelopeRequestSupervisorSilentRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_supervisor_silent_request`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeRequestSilentAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_silent_request`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeRequestSilentRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_silent_request`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeRequestSupervisorAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_supervisor_request`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeRequestSupervisorIdle`
- From: `Idle`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_supervisor_request`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Idle`

### `ClassifyExternalEnvelopeRequestSupervisorRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_supervisor_request`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeRequestActionableAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_actionable_request`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeRequestActionableRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_actionable_request`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeLifecycleAddedIdle`
- From: `Idle`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_lifecycle_added`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Idle`

### `ClassifyExternalEnvelopeLifecycleAddedAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_lifecycle_added`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeLifecycleAddedRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_lifecycle_added`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeLifecycleRetiredIdle`
- From: `Idle`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_lifecycle_retired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Idle`

### `ClassifyExternalEnvelopeLifecycleRetiredRetired`
- From: `Retired`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_lifecycle_retired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Retired`

### `ClassifyExternalEnvelopeLifecycleRetiredStopped`
- From: `Stopped`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_lifecycle_retired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Stopped`

### `ClassifyExternalEnvelopeLifecycleRetiredAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_lifecycle_retired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeLifecycleRetiredRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_lifecycle_retired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeLifecycleUnwiredIdle`
- From: `Idle`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_lifecycle_unwired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Idle`

### `ClassifyExternalEnvelopeLifecycleUnwiredRetired`
- From: `Retired`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_lifecycle_unwired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Retired`

### `ClassifyExternalEnvelopeLifecycleUnwiredStopped`
- From: `Stopped`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_lifecycle_unwired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Stopped`

### `ClassifyExternalEnvelopeLifecycleUnwiredAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_lifecycle_unwired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeLifecycleUnwiredRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_lifecycle_unwired`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeResponseAcceptedAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_response_accepted`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeResponseAcceptedRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_response_accepted`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeResponseCompletedAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_response_completed`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeResponseCompletedRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_response_completed`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeResponseFailedAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_response_failed`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeResponseFailedRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_response_failed`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Running`

### `ClassifyExternalEnvelopeAckAttached`
- From: `Attached`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
- Guards:
  - `session_registered`
  - `peer_ingress_ack`
- Emits: `EnqueueClassifiedEntry`, `PeerIngressClassified`
- To: `Attached`

### `ClassifyExternalEnvelopeAckRunning`
- From: `Running`
- On: `ClassifyExternalEnvelope`(item_id, from_peer, envelope_kind, request_intent, request_intent_class, lifecycle_kind, lifecycle_peer_param, response_status, in_reply_to)
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

### `ClassifyPeerResponseReplyAcceptedInitializing`
- From: `Initializing`
- On: `ClassifyPeerResponseReply`(status)
- Guards:
  - `peer_reply_status_accepted`
- Emits: `PeerResponseReplyClassified`
- To: `Initializing`

### `ClassifyPeerResponseReplyAcceptedIdle`
- From: `Idle`
- On: `ClassifyPeerResponseReply`(status)
- Guards:
  - `peer_reply_status_accepted`
- Emits: `PeerResponseReplyClassified`
- To: `Idle`

### `ClassifyPeerResponseReplyAcceptedAttached`
- From: `Attached`
- On: `ClassifyPeerResponseReply`(status)
- Guards:
  - `peer_reply_status_accepted`
- Emits: `PeerResponseReplyClassified`
- To: `Attached`

### `ClassifyPeerResponseReplyAcceptedRunning`
- From: `Running`
- On: `ClassifyPeerResponseReply`(status)
- Guards:
  - `peer_reply_status_accepted`
- Emits: `PeerResponseReplyClassified`
- To: `Running`

### `ClassifyPeerResponseReplyAcceptedRetired`
- From: `Retired`
- On: `ClassifyPeerResponseReply`(status)
- Guards:
  - `peer_reply_status_accepted`
- Emits: `PeerResponseReplyClassified`
- To: `Retired`

### `ClassifyPeerResponseReplyAcceptedStopped`
- From: `Stopped`
- On: `ClassifyPeerResponseReply`(status)
- Guards:
  - `peer_reply_status_accepted`
- Emits: `PeerResponseReplyClassified`
- To: `Stopped`

### `ClassifyPeerResponseReplyCompletedInitializing`
- From: `Initializing`
- On: `ClassifyPeerResponseReply`(status)
- Guards:
  - `peer_reply_status_completed`
- Emits: `PeerResponseReplyClassified`
- To: `Initializing`

### `ClassifyPeerResponseReplyCompletedIdle`
- From: `Idle`
- On: `ClassifyPeerResponseReply`(status)
- Guards:
  - `peer_reply_status_completed`
- Emits: `PeerResponseReplyClassified`
- To: `Idle`

### `ClassifyPeerResponseReplyCompletedAttached`
- From: `Attached`
- On: `ClassifyPeerResponseReply`(status)
- Guards:
  - `peer_reply_status_completed`
- Emits: `PeerResponseReplyClassified`
- To: `Attached`

### `ClassifyPeerResponseReplyCompletedRunning`
- From: `Running`
- On: `ClassifyPeerResponseReply`(status)
- Guards:
  - `peer_reply_status_completed`
- Emits: `PeerResponseReplyClassified`
- To: `Running`

### `ClassifyPeerResponseReplyCompletedRetired`
- From: `Retired`
- On: `ClassifyPeerResponseReply`(status)
- Guards:
  - `peer_reply_status_completed`
- Emits: `PeerResponseReplyClassified`
- To: `Retired`

### `ClassifyPeerResponseReplyCompletedStopped`
- From: `Stopped`
- On: `ClassifyPeerResponseReply`(status)
- Guards:
  - `peer_reply_status_completed`
- Emits: `PeerResponseReplyClassified`
- To: `Stopped`

### `ClassifyPeerResponseReplyFailedInitializing`
- From: `Initializing`
- On: `ClassifyPeerResponseReply`(status)
- Guards:
  - `peer_reply_status_failed`
- Emits: `PeerResponseReplyClassified`
- To: `Initializing`

### `ClassifyPeerResponseReplyFailedIdle`
- From: `Idle`
- On: `ClassifyPeerResponseReply`(status)
- Guards:
  - `peer_reply_status_failed`
- Emits: `PeerResponseReplyClassified`
- To: `Idle`

### `ClassifyPeerResponseReplyFailedAttached`
- From: `Attached`
- On: `ClassifyPeerResponseReply`(status)
- Guards:
  - `peer_reply_status_failed`
- Emits: `PeerResponseReplyClassified`
- To: `Attached`

### `ClassifyPeerResponseReplyFailedRunning`
- From: `Running`
- On: `ClassifyPeerResponseReply`(status)
- Guards:
  - `peer_reply_status_failed`
- Emits: `PeerResponseReplyClassified`
- To: `Running`

### `ClassifyPeerResponseReplyFailedRetired`
- From: `Retired`
- On: `ClassifyPeerResponseReply`(status)
- Guards:
  - `peer_reply_status_failed`
- Emits: `PeerResponseReplyClassified`
- To: `Retired`

### `ClassifyPeerResponseReplyFailedStopped`
- From: `Stopped`
- On: `ClassifyPeerResponseReply`(status)
- Guards:
  - `peer_reply_status_failed`
- Emits: `PeerResponseReplyClassified`
- To: `Stopped`

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

### `StartConversationRunIdleWithBinding`
- From: `Idle`
- On: `StartConversationRun`(run_id, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, max_extraction_retries)
- Guards:
  - `runtime_binding_present`
  - `turn_resettable`
  - `conversation_shape_matches_primitive`
- Emits: `TurnRunStarted`
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
- On: `PrimitiveApplied`(run_id)
- Guards:
  - `run_matches_current`
  - `turn_applying_conversation`
- Emits: `TurnCheckCompaction`
- To: `Running`

### `PrimitiveAppliedImmediateCompleted`
- From: `Running`
- On: `PrimitiveApplied`(run_id)
- Guards:
  - `run_matches_current`
  - `turn_applying_immediate`
  - `cancel_after_boundary_not_requested`
- Emits: `TurnBoundaryApplied`, `TurnRunCompleted`
- To: `Running`

### `PrimitiveAppliedImmediateCancelled`
- From: `Running`
- On: `PrimitiveApplied`(run_id)
- Guards:
  - `run_matches_current`
  - `turn_applying_immediate`
  - `cancel_after_boundary_requested`
- Emits: `TurnBoundaryApplied`, `TurnRunCancelled`
- To: `Running`

### `LlmReturnedToolCallsPositive`
- From: `Running`
- On: `LlmReturnedToolCalls`(run_id, tool_count)
- Guards:
  - `run_matches_current`
  - `turn_calling_llm`
  - `tool_count_positive`
- To: `Running`

### `LlmReturnedToolCallsZero`
- From: `Running`
- On: `LlmReturnedToolCalls`(run_id, tool_count)
- Guards:
  - `run_matches_current`
  - `turn_calling_llm`
  - `tool_count_zero`
- To: `Running`

### `LlmReturnedTerminal`
- From: `Running`
- On: `LlmReturnedTerminal`(run_id)
- Guards:
  - `run_matches_current`
  - `turn_calling_llm`
- To: `Running`

### `RegisterPendingOps`
- From: `Running`
- On: `RegisterPendingOps`(run_id, op_refs, barrier_operation_ids)
- Guards:
  - `run_matches_current`
  - `turn_waiting_or_calling`
- To: `Running`

### `ToolCallsResolvedToCalling`
- From: `Running`
- On: `ToolCallsResolved`(run_id)
- Guards:
  - `run_matches_current`
  - `turn_waiting_for_ops`
  - `barrier_not_satisfied`
- Emits: `TurnCheckCompaction`
- To: `Running`

### `ToolCallsResolvedToBoundary`
- From: `Running`
- On: `ToolCallsResolved`(run_id)
- Guards:
  - `run_matches_current`
  - `turn_waiting_for_ops`
  - `barrier_satisfied`
- To: `Running`

### `OpsBarrierSatisfied`
- From: `Running`
- On: `OpsBarrierSatisfied`(run_id, operation_ids)
- Guards:
  - `run_matches_current`
  - `turn_waiting_for_ops`
  - `matching_barrier_ids`
- To: `Running`

### `BoundaryContinueToCalling`
- From: `Running`
- On: `BoundaryContinue`(run_id)
- Guards:
  - `run_matches_current`
  - `turn_draining_boundary`
  - `cancel_after_boundary_not_requested`
- Emits: `TurnBoundaryApplied`, `TurnCheckCompaction`
- To: `Running`

### `BoundaryContinueToCancelled`
- From: `Running`
- On: `BoundaryContinue`(run_id)
- Guards:
  - `run_matches_current`
  - `turn_draining_boundary`
  - `cancel_after_boundary_requested`
- Emits: `TurnBoundaryApplied`, `TurnRunCancelled`
- To: `Running`

### `BoundaryCompleteCompleted`
- From: `Running`
- On: `BoundaryComplete`(run_id)
- Guards:
  - `run_matches_current`
  - `turn_draining_boundary`
  - `cancel_after_boundary_not_requested`
- Emits: `TurnBoundaryApplied`, `TurnRunCompleted`
- To: `Running`

### `BoundaryCompleteCancelled`
- From: `Running`
- On: `BoundaryComplete`(run_id)
- Guards:
  - `run_matches_current`
  - `turn_draining_boundary`
  - `cancel_after_boundary_requested`
- Emits: `TurnBoundaryApplied`, `TurnRunCancelled`
- To: `Running`

### `EnterExtraction`
- From: `Running`
- On: `EnterExtraction`(run_id, max_extraction_retries)
- Guards:
  - `run_matches_current`
  - `turn_draining_boundary`
- To: `Running`

### `ExtractionStart`
- From: `Running`
- On: `ExtractionStart`(run_id)
- Guards:
  - `run_matches_current`
  - `turn_extracting`
- Emits: `TurnCheckCompaction`
- To: `Running`

### `ExtractionValidationPassed`
- From: `Running`
- On: `ExtractionValidationPassed`(run_id)
- Guards:
  - `run_matches_current`
  - `turn_extracting`
- Emits: `TurnRunCompleted`
- To: `Running`

### `ExtractionValidationFailedRetry`
- From: `Running`
- On: `ExtractionValidationFailed`(run_id, error)
- Guards:
  - `run_matches_current`
  - `turn_extracting`
  - `retries_remaining`
- Emits: `TurnCheckCompaction`
- To: `Running`

### `ExtractionValidationFailedExhausted`
- From: `Running`
- On: `ExtractionValidationFailed`(run_id, error)
- Guards:
  - `run_matches_current`
  - `turn_extracting`
  - `retries_exhausted`
- Emits: `TurnRunCompleted`
- To: `Running`

### `ExtractionFailedTerminal`
- From: `Running`
- On: `ExtractionFailed`(run_id, error)
- Guards:
  - `run_matches_current`
  - `turn_extracting_calling_or_draining`
- Emits: `TurnRunCompleted`
- To: `Running`

### `RecoverableFailure`
- From: `Running`
- On: `RecoverableFailure`(run_id, failure_kind, retry_attempt, max_retries, selected_delay_ms, error)
- Guards:
  - `run_matches_current`
  - `turn_non_terminal`
  - `retry_attempt_present`
  - `failure_kind_recoverable`
  - `retries_remaining`
- To: `Running`

### `FatalFailure`
- From: `Running`
- On: `FatalFailure`(run_id, terminal_failure_source, error)
- Guards:
  - `run_matches_current`
  - `turn_not_terminal`
  - `terminal_failure_source_known`
- Emits: `TurnRunFailed`
- To: `Running`

### `RetryRequested`
- From: `Running`
- On: `RetryRequested`(run_id, retry_attempt)
- Guards:
  - `run_matches_current`
  - `turn_error_recovery`
  - `retry_attempt_matches`
- Emits: `TurnCheckCompaction`
- To: `Running`

### `CancelNow`
- From: `Running`
- On: `CancelNow`(run_id)
- Guards:
  - `run_matches_current`
  - `turn_cancellable`
- To: `Running`

### `RequestCancelAfterBoundary`
- From: `Running`
- On: `RequestCancelAfterBoundary`(run_id)
- Guards:
  - `run_matches_current`
  - `turn_cancellable`
- To: `Running`

### `CancellationObserved`
- From: `Running`
- On: `CancellationObserved`(run_id)
- Guards:
  - `run_matches_current`
  - `turn_cancelling`
- Emits: `TurnRunCancelled`
- To: `Running`

### `AcknowledgeTerminal`
- From: `Running`
- On: `AcknowledgeTerminal`(run_id, outcome)
- Guards:
  - `run_matches_current`
  - `turn_terminal`
- To: `Running`

### `TurnLimitReached`
- From: `Running`
- On: `TurnLimitReached`(run_id)
- Guards:
  - `run_matches_current`
  - `turn_not_terminal`
- Emits: `TurnRunFailed`
- To: `Running`

### `BudgetExhausted`
- From: `Running`
- On: `BudgetExhausted`(run_id)
- Guards:
  - `run_matches_current`
  - `turn_not_terminal`
- Emits: `TurnRunFailed`
- To: `Running`

### `TimeBudgetExceeded`
- From: `Running`
- On: `TimeBudgetExceeded`(run_id)
- Guards:
  - `run_matches_current`
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

### `ServiceTurnCommittedRunningToIdle`
- From: `Running`
- On: `ServiceTurnCommitted`(run_id)
- Guards:
  - `pre_run_phase_matches_idle`
  - `run_matches_binding`
  - `turn_completed`
- To: `Idle`

### `ServiceTurnCommittedRunningToAttached`
- From: `Running`
- On: `ServiceTurnCommitted`(run_id)
- Guards:
  - `pre_run_phase_matches_attached`
  - `run_matches_binding`
  - `turn_completed`
- To: `Attached`

### `ServiceTurnCommittedRunningToRetired`
- From: `Running`
- On: `ServiceTurnCommitted`(run_id)
- Guards:
  - `pre_run_phase_matches_retired`
  - `run_matches_binding`
  - `turn_completed`
- To: `Retired`

### `RunFailed`
- From: `Running`
- On: `RunFailed`(run_id, runtime_apply_failure_cause, runtime_apply_failure_message, machine_terminal_failure_observed, terminal_failure_source, error)
- Guards:
  - `run_matches_binding`
  - `runtime_apply_failure_not_machine_terminal_failure`
  - `machine_terminal_failure_not_raw_source`
  - `machine_terminal_failure_requires_existing_outcome`
  - `machine_terminal_failure_requires_existing_known_cause`
  - `machine_terminal_failure_existing_outcome_matches_cause`
  - `terminal_failure_source_known_if_present`
- Emits: `TurnRunFailed`, `PostAdmissionSignal`
- To: `Running`

### `RunCancelled`
- From: `Running`
- On: `RunCancelled`(run_id)
- Guards:
  - `run_matches_binding`
- To: `Running`

### `SurfaceRegisterIdle`
- From: `Idle`
- On: `SurfaceRegister`(surface_id)
- To: `Idle`

### `SurfaceRegisterAttached`
- From: `Attached`
- On: `SurfaceRegister`(surface_id)
- To: `Attached`

### `SurfaceRegisterRunning`
- From: `Running`
- On: `SurfaceRegister`(surface_id)
- To: `Running`

### `SurfaceStageAddIdle`
- From: `Idle`
- On: `SurfaceStageAdd`(surface_id, now_ms)
- Guards:
  - `surface_operating`
- To: `Idle`

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

### `SurfaceStageRemoveIdle`
- From: `Idle`
- On: `SurfaceStageRemove`(surface_id, now_ms)
- Guards:
  - `surface_operating`
- To: `Idle`

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

### `SurfaceSetRemovalTimeoutIdle`
- From: `Idle`
- On: `SurfaceSetRemovalTimeout`(timeout_ms)
- Guards:
  - `surface_operating`
- To: `Idle`

### `SurfaceSetRemovalTimeoutAttached`
- From: `Attached`
- On: `SurfaceSetRemovalTimeout`(timeout_ms)
- Guards:
  - `surface_operating`
- To: `Attached`

### `SurfaceSetRemovalTimeoutRunning`
- From: `Running`
- On: `SurfaceSetRemovalTimeout`(timeout_ms)
- Guards:
  - `surface_operating`
- To: `Running`

### `SurfaceStageReloadIdle`
- From: `Idle`
- On: `SurfaceStageReload`(surface_id, now_ms)
- Guards:
  - `surface_operating`
  - `surface_active`
- To: `Idle`

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

### `SurfaceApplyBoundaryAddIdle`
- From: `Idle`
- On: `SurfaceApplyBoundary`(surface_id, now_ms, staged_intent_sequence, applied_at_turn)
- Guards:
  - `surface_operating`
  - `staged_add`
  - `staged_sequence_matches`
  - `no_pending_surface_op`
  - `base_accepts_add`
- Emits: `ScheduleSurfaceCompletion`, `EmitExternalToolDelta`
- To: `Idle`

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

### `SurfaceApplyBoundaryReloadIdle`
- From: `Idle`
- On: `SurfaceApplyBoundary`(surface_id, now_ms, staged_intent_sequence, applied_at_turn)
- Guards:
  - `surface_operating`
  - `staged_reload`
  - `staged_sequence_matches`
  - `surface_active`
  - `base_active`
  - `no_pending_surface_op`
- Emits: `ScheduleSurfaceCompletion`, `EmitExternalToolDelta`
- To: `Idle`

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

### `SurfaceApplyBoundaryRemoveDrainingIdle`
- From: `Idle`
- On: `SurfaceApplyBoundary`(surface_id, now_ms, staged_intent_sequence, applied_at_turn)
- Guards:
  - `surface_operating`
  - `staged_remove`
  - `staged_sequence_matches`
  - `base_active`
  - `no_pending_surface_op`
- Emits: `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Idle`

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

### `SurfaceApplyBoundaryRemoveNoopIdle`
- From: `Idle`
- On: `SurfaceApplyBoundary`(surface_id, now_ms, staged_intent_sequence, applied_at_turn)
- Guards:
  - `surface_operating`
  - `staged_remove`
  - `staged_sequence_matches`
  - `base_not_active`
  - `no_pending_surface_op`
- To: `Idle`

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

### `SurfaceMarkPendingSucceededAddIdle`
- From: `Idle`
- On: `SurfaceMarkPendingSucceeded`(surface_id, pending_task_sequence, staged_intent_sequence)
- Guards:
  - `pending_add`
  - `pending_sequence_matches`
  - `pending_lineage_matches`
- Emits: `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Idle`

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

### `SurfaceMarkPendingSucceededReloadIdle`
- From: `Idle`
- On: `SurfaceMarkPendingSucceeded`(surface_id, pending_task_sequence, staged_intent_sequence)
- Guards:
  - `pending_reload`
  - `pending_sequence_matches`
  - `pending_lineage_matches`
- Emits: `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Idle`

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

### `SurfaceMarkPendingFailedIdle`
- From: `Idle`
- On: `SurfaceMarkPendingFailed`(surface_id, pending_task_sequence, staged_intent_sequence, cause)
- Guards:
  - `pending_sequence_matches`
  - `pending_lineage_matches`
- Emits: `EmitExternalToolDelta`
- To: `Idle`

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

### `SurfaceFinalizeRemovalCleanIdle`
- From: `Idle`
- On: `SurfaceFinalizeRemovalClean`(surface_id)
- Guards:
  - `surface_removing`
  - `no_inflight_calls_remain`
- Emits: `CloseSurfaceConnection`, `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Idle`

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

### `SurfaceFinalizeRemovalForcedIdle`
- From: `Idle`
- On: `SurfaceFinalizeRemovalForced`(surface_id)
- Guards:
  - `surface_removing`
- Emits: `CloseSurfaceConnection`, `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Idle`

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

### `SurfaceSnapshotAlignedIdle`
- From: `Idle`
- On: `SurfaceSnapshotAligned`(epoch)
- To: `Idle`

### `SurfaceSnapshotAlignedAttached`
- From: `Attached`
- On: `SurfaceSnapshotAligned`(epoch)
- To: `Attached`

### `SurfaceSnapshotAlignedRunning`
- From: `Running`
- On: `SurfaceSnapshotAligned`(epoch)
- To: `Running`

### `SurfaceShutdownIdle`
- From: `Idle`
- On: `SurfaceShutdown`()
- To: `Idle`

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

### `CancelRunningToIdle`
- From: `Running`
- On: `CancelRun`(run_id)
- Guards:
  - `pre_run_phase_matches_idle`
  - `current_run_id_matches_binding`
  - `turn_cancelled`
- Emits: `RecordTerminalOutcome`
- To: `Idle`

### `CancelRunningToAttached`
- From: `Running`
- On: `CancelRun`(run_id)
- Guards:
  - `pre_run_phase_matches_attached`
  - `current_run_id_matches_binding`
  - `turn_cancelled`
- Emits: `RecordTerminalOutcome`
- To: `Attached`

### `CancelRunningToRetired`
- From: `Running`
- On: `CancelRun`(run_id)
- Guards:
  - `pre_run_phase_matches_retired`
  - `current_run_id_matches_binding`
  - `turn_cancelled`
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

### `RecoverAdmittedInputIdle`
- From: `Idle`
- On: `RecoverAdmittedInput`(input_id, input_kind, runtime_boundary, runtime_execution_kind, runtime_peer_response_terminal_apply_intent, lane)
- Guards:
  - `recovered_execution_kind_matches_input`
  - `recovered_terminal_intent_matches_input`
  - `recovered_immediate_boundary_uses_steer_lane`
- To: `Idle`

### `RecoverAdmittedInputAttached`
- From: `Attached`
- On: `RecoverAdmittedInput`(input_id, input_kind, runtime_boundary, runtime_execution_kind, runtime_peer_response_terminal_apply_intent, lane)
- Guards:
  - `recovered_execution_kind_matches_input`
  - `recovered_terminal_intent_matches_input`
  - `recovered_immediate_boundary_uses_steer_lane`
- To: `Attached`

### `RecoverAdmittedInputRunning`
- From: `Running`
- On: `RecoverAdmittedInput`(input_id, input_kind, runtime_boundary, runtime_execution_kind, runtime_peer_response_terminal_apply_intent, lane)
- Guards:
  - `recovered_execution_kind_matches_input`
  - `recovered_terminal_intent_matches_input`
  - `recovered_immediate_boundary_uses_steer_lane`
- To: `Running`

### `RecoverAdmittedInputRetired`
- From: `Retired`
- On: `RecoverAdmittedInput`(input_id, input_kind, runtime_boundary, runtime_execution_kind, runtime_peer_response_terminal_apply_intent, lane)
- Guards:
  - `recovered_execution_kind_matches_input`
  - `recovered_terminal_intent_matches_input`
  - `recovered_immediate_boundary_uses_steer_lane`
- To: `Retired`

### `RecoverAdmittedInputStopped`
- From: `Stopped`
- On: `RecoverAdmittedInput`(input_id, input_kind, runtime_boundary, runtime_execution_kind, runtime_peer_response_terminal_apply_intent, lane)
- Guards:
  - `recovered_execution_kind_matches_input`
  - `recovered_terminal_intent_matches_input`
  - `recovered_immediate_boundary_uses_steer_lane`
- To: `Stopped`

### `RecoverInputLifecycleIdle`
- From: `Idle`
- On: `RecoverInputLifecycle`(input_id, phase, terminal_kind, superseded_by, aggregate_id, abandon_reason, abandon_attempt_count, attempt_count, run_id, boundary_sequence, admission_sequence, admission_sequence_recovery, recovery_lane, lane)
- Guards:
  - `recovered_lifecycle_has_admission_witness`
  - `recovered_recovery_lane_matches_witness`
  - `recovered_current_lane_matches_phase`
  - `recovered_queued_order_has_witness`
  - `recovered_order_recovery_matches_missing_sequence`
  - `recovered_terminal_payload_matches_phase`
  - `recovered_max_attempts_reason_matches_count`
  - `recovered_max_attempts_reason_matches_policy`
- Emits: `InputLifecycleNotice`
- To: `Idle`

### `RecoverInputLifecycleAttached`
- From: `Attached`
- On: `RecoverInputLifecycle`(input_id, phase, terminal_kind, superseded_by, aggregate_id, abandon_reason, abandon_attempt_count, attempt_count, run_id, boundary_sequence, admission_sequence, admission_sequence_recovery, recovery_lane, lane)
- Guards:
  - `recovered_lifecycle_has_admission_witness`
  - `recovered_recovery_lane_matches_witness`
  - `recovered_current_lane_matches_phase`
  - `recovered_queued_order_has_witness`
  - `recovered_order_recovery_matches_missing_sequence`
  - `recovered_terminal_payload_matches_phase`
  - `recovered_max_attempts_reason_matches_count`
  - `recovered_max_attempts_reason_matches_policy`
- Emits: `InputLifecycleNotice`
- To: `Attached`

### `RecoverInputLifecycleRunning`
- From: `Running`
- On: `RecoverInputLifecycle`(input_id, phase, terminal_kind, superseded_by, aggregate_id, abandon_reason, abandon_attempt_count, attempt_count, run_id, boundary_sequence, admission_sequence, admission_sequence_recovery, recovery_lane, lane)
- Guards:
  - `recovered_lifecycle_has_admission_witness`
  - `recovered_recovery_lane_matches_witness`
  - `recovered_current_lane_matches_phase`
  - `recovered_queued_order_has_witness`
  - `recovered_order_recovery_matches_missing_sequence`
  - `recovered_terminal_payload_matches_phase`
  - `recovered_max_attempts_reason_matches_count`
  - `recovered_max_attempts_reason_matches_policy`
- Emits: `InputLifecycleNotice`
- To: `Running`

### `RecoverInputLifecycleRetired`
- From: `Retired`
- On: `RecoverInputLifecycle`(input_id, phase, terminal_kind, superseded_by, aggregate_id, abandon_reason, abandon_attempt_count, attempt_count, run_id, boundary_sequence, admission_sequence, admission_sequence_recovery, recovery_lane, lane)
- Guards:
  - `recovered_lifecycle_has_admission_witness`
  - `recovered_recovery_lane_matches_witness`
  - `recovered_current_lane_matches_phase`
  - `recovered_queued_order_has_witness`
  - `recovered_order_recovery_matches_missing_sequence`
  - `recovered_terminal_payload_matches_phase`
  - `recovered_max_attempts_reason_matches_count`
  - `recovered_max_attempts_reason_matches_policy`
- Emits: `InputLifecycleNotice`
- To: `Retired`

### `RecoverInputLifecycleStopped`
- From: `Stopped`
- On: `RecoverInputLifecycle`(input_id, phase, terminal_kind, superseded_by, aggregate_id, abandon_reason, abandon_attempt_count, attempt_count, run_id, boundary_sequence, admission_sequence, admission_sequence_recovery, recovery_lane, lane)
- Guards:
  - `recovered_lifecycle_has_admission_witness`
  - `recovered_recovery_lane_matches_witness`
  - `recovered_current_lane_matches_phase`
  - `recovered_queued_order_has_witness`
  - `recovered_order_recovery_matches_missing_sequence`
  - `recovered_terminal_payload_matches_phase`
  - `recovered_max_attempts_reason_matches_count`
  - `recovered_max_attempts_reason_matches_policy`
- Emits: `InputLifecycleNotice`
- To: `Stopped`

### `QueueAcceptedIdle`
- From: `Idle`
- On: `QueueAccepted`(input_id)
- Guards:
  - `not_already_tracked`
  - `live_admission_authorized_queue_lane`
- Emits: `IngressAccepted`
- To: `Idle`

### `QueueAcceptedAttached`
- From: `Attached`
- On: `QueueAccepted`(input_id)
- Guards:
  - `not_already_tracked`
  - `live_admission_authorized_queue_lane`
- Emits: `IngressAccepted`
- To: `Attached`

### `QueueAcceptedRunning`
- From: `Running`
- On: `QueueAccepted`(input_id)
- Guards:
  - `not_already_tracked`
  - `live_admission_authorized_queue_lane`
- Emits: `IngressAccepted`
- To: `Running`

### `QueueAcceptedRetired`
- From: `Retired`
- On: `QueueAccepted`(input_id)
- Guards:
  - `not_already_tracked`
  - `live_admission_authorized_queue_lane`
- Emits: `IngressAccepted`
- To: `Retired`

### `QueueAcceptedStopped`
- From: `Stopped`
- On: `QueueAccepted`(input_id)
- Guards:
  - `not_already_tracked`
  - `live_admission_authorized_queue_lane`
- Emits: `IngressAccepted`
- To: `Stopped`

### `SteerAcceptedIdle`
- From: `Idle`
- On: `SteerAccepted`(input_id)
- Guards:
  - `not_already_tracked`
  - `live_admission_authorized_steer_lane`
- Emits: `IngressAccepted`
- To: `Idle`

### `SteerAcceptedAttached`
- From: `Attached`
- On: `SteerAccepted`(input_id)
- Guards:
  - `not_already_tracked`
  - `live_admission_authorized_steer_lane`
- Emits: `IngressAccepted`
- To: `Attached`

### `SteerAcceptedRunning`
- From: `Running`
- On: `SteerAccepted`(input_id)
- Guards:
  - `not_already_tracked`
  - `live_admission_authorized_steer_lane`
- Emits: `IngressAccepted`
- To: `Running`

### `SteerAcceptedRetired`
- From: `Retired`
- On: `SteerAccepted`(input_id)
- Guards:
  - `not_already_tracked`
  - `live_admission_authorized_steer_lane`
- Emits: `IngressAccepted`
- To: `Retired`

### `SteerAcceptedStopped`
- From: `Stopped`
- On: `SteerAccepted`(input_id)
- Guards:
  - `not_already_tracked`
  - `live_admission_authorized_steer_lane`
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

### `PrioritizeInputIdle`
- From: `Idle`
- On: `PrioritizeInput`(input_id)
- Guards:
  - `input_queued`
  - `priority_sequence_available`
- To: `Idle`

### `PrioritizeInputAttached`
- From: `Attached`
- On: `PrioritizeInput`(input_id)
- Guards:
  - `input_queued`
  - `priority_sequence_available`
- To: `Attached`

### `PrioritizeInputRunning`
- From: `Running`
- On: `PrioritizeInput`(input_id)
- Guards:
  - `input_queued`
  - `priority_sequence_available`
- To: `Running`

### `PrioritizeInputRetired`
- From: `Retired`
- On: `PrioritizeInput`(input_id)
- Guards:
  - `input_queued`
  - `priority_sequence_available`
- To: `Retired`

### `PrioritizeInputStopped`
- From: `Stopped`
- On: `PrioritizeInput`(input_id)
- Guards:
  - `input_queued`
  - `priority_sequence_available`
- To: `Stopped`

### `DeferInputBehindBacklogIdle`
- From: `Idle`
- On: `DeferInputBehindBacklog`(input_id)
- Guards:
  - `input_queued`
- To: `Idle`

### `DeferInputBehindBacklogAttached`
- From: `Attached`
- On: `DeferInputBehindBacklog`(input_id)
- Guards:
  - `input_queued`
- To: `Attached`

### `DeferInputBehindBacklogRunning`
- From: `Running`
- On: `DeferInputBehindBacklog`(input_id)
- Guards:
  - `input_queued`
- To: `Running`

### `DeferInputBehindBacklogRetired`
- From: `Retired`
- On: `DeferInputBehindBacklog`(input_id)
- Guards:
  - `input_queued`
- To: `Retired`

### `DeferInputBehindBacklogStopped`
- From: `Stopped`
- On: `DeferInputBehindBacklog`(input_id)
- Guards:
  - `input_queued`
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

### `ResolveStagedRollbackQueuedIdle`
- From: `Idle`
- On: `ResolveStagedRollback`(input_id, lane)
- Guards:
  - `input_tracked`
  - `input_staged`
  - `attempt_count_tracked`
  - `recovery_lane_matches`
  - `stage_attempts_remaining`
- Emits: `InputLifecycleNotice`
- To: `Idle`

### `ResolveStagedRollbackQueuedAttached`
- From: `Attached`
- On: `ResolveStagedRollback`(input_id, lane)
- Guards:
  - `input_tracked`
  - `input_staged`
  - `attempt_count_tracked`
  - `recovery_lane_matches`
  - `stage_attempts_remaining`
- Emits: `InputLifecycleNotice`
- To: `Attached`

### `ResolveStagedRollbackQueuedRunning`
- From: `Running`
- On: `ResolveStagedRollback`(input_id, lane)
- Guards:
  - `input_tracked`
  - `input_staged`
  - `attempt_count_tracked`
  - `recovery_lane_matches`
  - `stage_attempts_remaining`
- Emits: `InputLifecycleNotice`
- To: `Running`

### `ResolveStagedRollbackQueuedRetired`
- From: `Retired`
- On: `ResolveStagedRollback`(input_id, lane)
- Guards:
  - `input_tracked`
  - `input_staged`
  - `attempt_count_tracked`
  - `recovery_lane_matches`
  - `stage_attempts_remaining`
- Emits: `InputLifecycleNotice`
- To: `Retired`

### `ResolveStagedRollbackQueuedStopped`
- From: `Stopped`
- On: `ResolveStagedRollback`(input_id, lane)
- Guards:
  - `input_tracked`
  - `input_staged`
  - `attempt_count_tracked`
  - `recovery_lane_matches`
  - `stage_attempts_remaining`
- Emits: `InputLifecycleNotice`
- To: `Stopped`

### `ResolveStagedRollbackMaxAttemptsExhaustedIdle`
- From: `Idle`
- On: `ResolveStagedRollback`(input_id, lane)
- Guards:
  - `input_tracked`
  - `input_staged`
  - `attempt_count_tracked`
  - `recovery_lane_matches`
  - `stage_attempts_exhausted`
- Emits: `RecordTerminalOutcome`
- To: `Idle`

### `ResolveStagedRollbackMaxAttemptsExhaustedAttached`
- From: `Attached`
- On: `ResolveStagedRollback`(input_id, lane)
- Guards:
  - `input_tracked`
  - `input_staged`
  - `attempt_count_tracked`
  - `recovery_lane_matches`
  - `stage_attempts_exhausted`
- Emits: `RecordTerminalOutcome`
- To: `Attached`

### `ResolveStagedRollbackMaxAttemptsExhaustedRunning`
- From: `Running`
- On: `ResolveStagedRollback`(input_id, lane)
- Guards:
  - `input_tracked`
  - `input_staged`
  - `attempt_count_tracked`
  - `recovery_lane_matches`
  - `stage_attempts_exhausted`
- Emits: `RecordTerminalOutcome`
- To: `Running`

### `ResolveStagedRollbackMaxAttemptsExhaustedRetired`
- From: `Retired`
- On: `ResolveStagedRollback`(input_id, lane)
- Guards:
  - `input_tracked`
  - `input_staged`
  - `attempt_count_tracked`
  - `recovery_lane_matches`
  - `stage_attempts_exhausted`
- Emits: `RecordTerminalOutcome`
- To: `Retired`

### `ResolveStagedRollbackMaxAttemptsExhaustedStopped`
- From: `Stopped`
- On: `ResolveStagedRollback`(input_id, lane)
- Guards:
  - `input_tracked`
  - `input_staged`
  - `attempt_count_tracked`
  - `recovery_lane_matches`
  - `stage_attempts_exhausted`
- Emits: `RecordTerminalOutcome`
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

### `ResolveLiveBoundaryContextReceiptRunning`
- From: `Running`
- On: `ResolveLiveBoundaryContextReceipt`(run_id, input_id)
- Guards:
  - `current_run_matches`
  - `input_tracked`
- Emits: `LiveBoundaryContextReceiptResolved`
- To: `Running`

### `ConsumeOnAcceptIdle`
- From: `Idle`
- On: `ConsumeOnAccept`(input_id)
- Guards:
  - `input_tracked`
  - `live_admission_authorized_consume_on_accept`
- Emits: `RecordTerminalOutcome`
- To: `Idle`

### `ConsumeOnAcceptAttached`
- From: `Attached`
- On: `ConsumeOnAccept`(input_id)
- Guards:
  - `input_tracked`
  - `live_admission_authorized_consume_on_accept`
- Emits: `RecordTerminalOutcome`
- To: `Attached`

### `ConsumeOnAcceptRunning`
- From: `Running`
- On: `ConsumeOnAccept`(input_id)
- Guards:
  - `input_tracked`
  - `live_admission_authorized_consume_on_accept`
- Emits: `RecordTerminalOutcome`
- To: `Running`

### `ConsumeOnAcceptRetired`
- From: `Retired`
- On: `ConsumeOnAccept`(input_id)
- Guards:
  - `input_tracked`
  - `live_admission_authorized_consume_on_accept`
- Emits: `RecordTerminalOutcome`
- To: `Retired`

### `ConsumeOnAcceptStopped`
- From: `Stopped`
- On: `ConsumeOnAccept`(input_id)
- Guards:
  - `input_tracked`
  - `live_admission_authorized_consume_on_accept`
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
  - `live_admission_authorized_supersede_target`
- Emits: `RecordTerminalOutcome`
- To: `Idle`

### `SupersedeInputAttached`
- From: `Attached`
- On: `SupersedeInput`(input_id, superseded_by)
- Guards:
  - `input_tracked`
  - `live_admission_authorized_supersede_target`
- Emits: `RecordTerminalOutcome`
- To: `Attached`

### `SupersedeInputRunning`
- From: `Running`
- On: `SupersedeInput`(input_id, superseded_by)
- Guards:
  - `input_tracked`
  - `live_admission_authorized_supersede_target`
- Emits: `RecordTerminalOutcome`
- To: `Running`

### `SupersedeInputRetired`
- From: `Retired`
- On: `SupersedeInput`(input_id, superseded_by)
- Guards:
  - `input_tracked`
  - `live_admission_authorized_supersede_target`
- Emits: `RecordTerminalOutcome`
- To: `Retired`

### `SupersedeInputStopped`
- From: `Stopped`
- On: `SupersedeInput`(input_id, superseded_by)
- Guards:
  - `input_tracked`
  - `live_admission_authorized_supersede_target`
- Emits: `RecordTerminalOutcome`
- To: `Stopped`

### `CoalesceInputIdle`
- From: `Idle`
- On: `CoalesceInput`(input_id, aggregate_id)
- Guards:
  - `input_tracked`
  - `live_admission_authorized_coalesce_target`
- Emits: `RecordTerminalOutcome`
- To: `Idle`

### `CoalesceInputAttached`
- From: `Attached`
- On: `CoalesceInput`(input_id, aggregate_id)
- Guards:
  - `input_tracked`
  - `live_admission_authorized_coalesce_target`
- Emits: `RecordTerminalOutcome`
- To: `Attached`

### `CoalesceInputRunning`
- From: `Running`
- On: `CoalesceInput`(input_id, aggregate_id)
- Guards:
  - `input_tracked`
  - `live_admission_authorized_coalesce_target`
- Emits: `RecordTerminalOutcome`
- To: `Running`

### `CoalesceInputRetired`
- From: `Retired`
- On: `CoalesceInput`(input_id, aggregate_id)
- Guards:
  - `input_tracked`
  - `live_admission_authorized_coalesce_target`
- Emits: `RecordTerminalOutcome`
- To: `Retired`

### `CoalesceInputStopped`
- From: `Stopped`
- On: `CoalesceInput`(input_id, aggregate_id)
- Guards:
  - `input_tracked`
  - `live_admission_authorized_coalesce_target`
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

### `RegisterOpAlreadyRegisteredRejectedIdle`
- From: `Idle`
- On: `RegisterOp`(operation_id, kind, source, max_concurrent)
- Guards:
  - `already_registered`
- Emits: `OpRegistrationAdmissionResolved`
- To: `Idle`

### `RegisterOpAlreadyRegisteredRejectedAttached`
- From: `Attached`
- On: `RegisterOp`(operation_id, kind, source, max_concurrent)
- Guards:
  - `already_registered`
- Emits: `OpRegistrationAdmissionResolved`
- To: `Attached`

### `RegisterOpAlreadyRegisteredRejectedRunning`
- From: `Running`
- On: `RegisterOp`(operation_id, kind, source, max_concurrent)
- Guards:
  - `already_registered`
- Emits: `OpRegistrationAdmissionResolved`
- To: `Running`

### `RegisterOpAlreadyRegisteredRejectedRetired`
- From: `Retired`
- On: `RegisterOp`(operation_id, kind, source, max_concurrent)
- Guards:
  - `already_registered`
- Emits: `OpRegistrationAdmissionResolved`
- To: `Retired`

### `RegisterOpAlreadyRegisteredRejectedStopped`
- From: `Stopped`
- On: `RegisterOp`(operation_id, kind, source, max_concurrent)
- Guards:
  - `already_registered`
- Emits: `OpRegistrationAdmissionResolved`
- To: `Stopped`

### `RegisterOpMaxConcurrentRejectedIdle`
- From: `Idle`
- On: `RegisterOp`(operation_id, kind, source, max_concurrent)
- Guards:
  - `not_already_registered`
  - `operation_source_valid`
  - `max_concurrent_present`
  - `max_concurrent_exceeded`
- Emits: `OpRegistrationAdmissionResolved`
- To: `Idle`

### `RegisterOpMaxConcurrentRejectedAttached`
- From: `Attached`
- On: `RegisterOp`(operation_id, kind, source, max_concurrent)
- Guards:
  - `not_already_registered`
  - `operation_source_valid`
  - `max_concurrent_present`
  - `max_concurrent_exceeded`
- Emits: `OpRegistrationAdmissionResolved`
- To: `Attached`

### `RegisterOpMaxConcurrentRejectedRunning`
- From: `Running`
- On: `RegisterOp`(operation_id, kind, source, max_concurrent)
- Guards:
  - `not_already_registered`
  - `operation_source_valid`
  - `max_concurrent_present`
  - `max_concurrent_exceeded`
- Emits: `OpRegistrationAdmissionResolved`
- To: `Running`

### `RegisterOpMaxConcurrentRejectedRetired`
- From: `Retired`
- On: `RegisterOp`(operation_id, kind, source, max_concurrent)
- Guards:
  - `not_already_registered`
  - `operation_source_valid`
  - `max_concurrent_present`
  - `max_concurrent_exceeded`
- Emits: `OpRegistrationAdmissionResolved`
- To: `Retired`

### `RegisterOpMaxConcurrentRejectedStopped`
- From: `Stopped`
- On: `RegisterOp`(operation_id, kind, source, max_concurrent)
- Guards:
  - `not_already_registered`
  - `operation_source_valid`
  - `max_concurrent_present`
  - `max_concurrent_exceeded`
- Emits: `OpRegistrationAdmissionResolved`
- To: `Stopped`

### `RegisterOpAcceptedIdle`
- From: `Idle`
- On: `RegisterOp`(operation_id, kind, source, max_concurrent)
- Guards:
  - `not_already_registered`
  - `operation_source_valid`
  - `capacity_available`
- Emits: `OpRegistrationAdmissionResolved`, `SubmitOpEvent`
- To: `Idle`

### `RegisterOpAcceptedAttached`
- From: `Attached`
- On: `RegisterOp`(operation_id, kind, source, max_concurrent)
- Guards:
  - `not_already_registered`
  - `operation_source_valid`
  - `capacity_available`
- Emits: `OpRegistrationAdmissionResolved`, `SubmitOpEvent`
- To: `Attached`

### `RegisterOpAcceptedRunning`
- From: `Running`
- On: `RegisterOp`(operation_id, kind, source, max_concurrent)
- Guards:
  - `not_already_registered`
  - `operation_source_valid`
  - `capacity_available`
- Emits: `OpRegistrationAdmissionResolved`, `SubmitOpEvent`
- To: `Running`

### `RegisterOpAcceptedRetired`
- From: `Retired`
- On: `RegisterOp`(operation_id, kind, source, max_concurrent)
- Guards:
  - `not_already_registered`
  - `operation_source_valid`
  - `capacity_available`
- Emits: `OpRegistrationAdmissionResolved`, `SubmitOpEvent`
- To: `Retired`

### `RegisterOpAcceptedStopped`
- From: `Stopped`
- On: `RegisterOp`(operation_id, kind, source, max_concurrent)
- Guards:
  - `not_already_registered`
  - `operation_source_valid`
  - `capacity_available`
- Emits: `OpRegistrationAdmissionResolved`, `SubmitOpEvent`
- To: `Stopped`

### `ResolveOpLifecycleTransitionNotFoundRejectedIdle`
- From: `Idle`
- On: `ResolveOpLifecycleTransitionRejection`(operation_id, action)
- Guards:
  - `op_not_registered`
- Emits: `OpLifecycleTransitionRejected`
- To: `Idle`

### `ResolveOpLifecycleTransitionNotFoundRejectedAttached`
- From: `Attached`
- On: `ResolveOpLifecycleTransitionRejection`(operation_id, action)
- Guards:
  - `op_not_registered`
- Emits: `OpLifecycleTransitionRejected`
- To: `Attached`

### `ResolveOpLifecycleTransitionNotFoundRejectedRunning`
- From: `Running`
- On: `ResolveOpLifecycleTransitionRejection`(operation_id, action)
- Guards:
  - `op_not_registered`
- Emits: `OpLifecycleTransitionRejected`
- To: `Running`

### `ResolveOpLifecycleTransitionNotFoundRejectedRetired`
- From: `Retired`
- On: `ResolveOpLifecycleTransitionRejection`(operation_id, action)
- Guards:
  - `op_not_registered`
- Emits: `OpLifecycleTransitionRejected`
- To: `Retired`

### `ResolveOpLifecycleTransitionNotFoundRejectedStopped`
- From: `Stopped`
- On: `ResolveOpLifecycleTransitionRejection`(operation_id, action)
- Guards:
  - `op_not_registered`
- Emits: `OpLifecycleTransitionRejected`
- To: `Stopped`

### `ResolveOpLifecycleTransitionPeerNotExpectedRejectedIdle`
- From: `Idle`
- On: `ResolveOpLifecycleTransitionRejection`(operation_id, action)
- Guards:
  - `op_registered`
  - `action_peer_ready`
  - `kind_not_mob_member_child`
- Emits: `OpLifecycleTransitionRejected`
- To: `Idle`

### `ResolveOpLifecycleTransitionPeerNotExpectedRejectedAttached`
- From: `Attached`
- On: `ResolveOpLifecycleTransitionRejection`(operation_id, action)
- Guards:
  - `op_registered`
  - `action_peer_ready`
  - `kind_not_mob_member_child`
- Emits: `OpLifecycleTransitionRejected`
- To: `Attached`

### `ResolveOpLifecycleTransitionPeerNotExpectedRejectedRunning`
- From: `Running`
- On: `ResolveOpLifecycleTransitionRejection`(operation_id, action)
- Guards:
  - `op_registered`
  - `action_peer_ready`
  - `kind_not_mob_member_child`
- Emits: `OpLifecycleTransitionRejected`
- To: `Running`

### `ResolveOpLifecycleTransitionPeerNotExpectedRejectedRetired`
- From: `Retired`
- On: `ResolveOpLifecycleTransitionRejection`(operation_id, action)
- Guards:
  - `op_registered`
  - `action_peer_ready`
  - `kind_not_mob_member_child`
- Emits: `OpLifecycleTransitionRejected`
- To: `Retired`

### `ResolveOpLifecycleTransitionPeerNotExpectedRejectedStopped`
- From: `Stopped`
- On: `ResolveOpLifecycleTransitionRejection`(operation_id, action)
- Guards:
  - `op_registered`
  - `action_peer_ready`
  - `kind_not_mob_member_child`
- Emits: `OpLifecycleTransitionRejected`
- To: `Stopped`

### `ResolveOpLifecycleTransitionAlreadyPeerReadyRejectedIdle`
- From: `Idle`
- On: `ResolveOpLifecycleTransitionRejection`(operation_id, action)
- Guards:
  - `op_registered`
  - `action_peer_ready`
  - `kind_is_mob_member_child`
  - `already_peer_ready`
- Emits: `OpLifecycleTransitionRejected`
- To: `Idle`

### `ResolveOpLifecycleTransitionAlreadyPeerReadyRejectedAttached`
- From: `Attached`
- On: `ResolveOpLifecycleTransitionRejection`(operation_id, action)
- Guards:
  - `op_registered`
  - `action_peer_ready`
  - `kind_is_mob_member_child`
  - `already_peer_ready`
- Emits: `OpLifecycleTransitionRejected`
- To: `Attached`

### `ResolveOpLifecycleTransitionAlreadyPeerReadyRejectedRunning`
- From: `Running`
- On: `ResolveOpLifecycleTransitionRejection`(operation_id, action)
- Guards:
  - `op_registered`
  - `action_peer_ready`
  - `kind_is_mob_member_child`
  - `already_peer_ready`
- Emits: `OpLifecycleTransitionRejected`
- To: `Running`

### `ResolveOpLifecycleTransitionAlreadyPeerReadyRejectedRetired`
- From: `Retired`
- On: `ResolveOpLifecycleTransitionRejection`(operation_id, action)
- Guards:
  - `op_registered`
  - `action_peer_ready`
  - `kind_is_mob_member_child`
  - `already_peer_ready`
- Emits: `OpLifecycleTransitionRejected`
- To: `Retired`

### `ResolveOpLifecycleTransitionAlreadyPeerReadyRejectedStopped`
- From: `Stopped`
- On: `ResolveOpLifecycleTransitionRejection`(operation_id, action)
- Guards:
  - `op_registered`
  - `action_peer_ready`
  - `kind_is_mob_member_child`
  - `already_peer_ready`
- Emits: `OpLifecycleTransitionRejected`
- To: `Stopped`

### `ResolveOpLifecycleTransitionInvalidRejectedIdle`
- From: `Idle`
- On: `ResolveOpLifecycleTransitionRejection`(operation_id, action)
- Guards:
  - `op_registered`
  - `peer_ready_special_rejections_absent`
  - `from_status_invalid`
- Emits: `OpLifecycleTransitionRejected`
- To: `Idle`

### `ResolveOpLifecycleTransitionInvalidRejectedAttached`
- From: `Attached`
- On: `ResolveOpLifecycleTransitionRejection`(operation_id, action)
- Guards:
  - `op_registered`
  - `peer_ready_special_rejections_absent`
  - `from_status_invalid`
- Emits: `OpLifecycleTransitionRejected`
- To: `Attached`

### `ResolveOpLifecycleTransitionInvalidRejectedRunning`
- From: `Running`
- On: `ResolveOpLifecycleTransitionRejection`(operation_id, action)
- Guards:
  - `op_registered`
  - `peer_ready_special_rejections_absent`
  - `from_status_invalid`
- Emits: `OpLifecycleTransitionRejected`
- To: `Running`

### `ResolveOpLifecycleTransitionInvalidRejectedRetired`
- From: `Retired`
- On: `ResolveOpLifecycleTransitionRejection`(operation_id, action)
- Guards:
  - `op_registered`
  - `peer_ready_special_rejections_absent`
  - `from_status_invalid`
- Emits: `OpLifecycleTransitionRejected`
- To: `Retired`

### `ResolveOpLifecycleTransitionInvalidRejectedStopped`
- From: `Stopped`
- On: `ResolveOpLifecycleTransitionRejection`(operation_id, action)
- Guards:
  - `op_registered`
  - `peer_ready_special_rejections_absent`
  - `from_status_invalid`
- Emits: `OpLifecycleTransitionRejected`
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

### `RecoverOpRecordIdle`
- From: `Idle`
- On: `RecoverOpRecord`(operation_id, status, kind, source, peer_ready, progress_count, terminal_outcome, terminal_payload, completion_sequence)
- Guards:
  - `not_already_registered`
  - `recovered_status_terminal`
  - `terminal_outcome_present`
  - `terminal_payload_present`
  - `completion_sequence_present`
  - `operation_source_valid`
  - `completion_sequence_unclaimed`
  - `terminal_outcome_matches_status`
- Emits: `RetainTerminalRecord`
- To: `Idle`

### `RecoverOpRecordAttached`
- From: `Attached`
- On: `RecoverOpRecord`(operation_id, status, kind, source, peer_ready, progress_count, terminal_outcome, terminal_payload, completion_sequence)
- Guards:
  - `not_already_registered`
  - `recovered_status_terminal`
  - `terminal_outcome_present`
  - `terminal_payload_present`
  - `completion_sequence_present`
  - `operation_source_valid`
  - `completion_sequence_unclaimed`
  - `terminal_outcome_matches_status`
- Emits: `RetainTerminalRecord`
- To: `Attached`

### `RecoverOpRecordRunning`
- From: `Running`
- On: `RecoverOpRecord`(operation_id, status, kind, source, peer_ready, progress_count, terminal_outcome, terminal_payload, completion_sequence)
- Guards:
  - `not_already_registered`
  - `recovered_status_terminal`
  - `terminal_outcome_present`
  - `terminal_payload_present`
  - `completion_sequence_present`
  - `operation_source_valid`
  - `completion_sequence_unclaimed`
  - `terminal_outcome_matches_status`
- Emits: `RetainTerminalRecord`
- To: `Running`

### `RecoverOpRecordRetired`
- From: `Retired`
- On: `RecoverOpRecord`(operation_id, status, kind, source, peer_ready, progress_count, terminal_outcome, terminal_payload, completion_sequence)
- Guards:
  - `not_already_registered`
  - `recovered_status_terminal`
  - `terminal_outcome_present`
  - `terminal_payload_present`
  - `completion_sequence_present`
  - `operation_source_valid`
  - `completion_sequence_unclaimed`
  - `terminal_outcome_matches_status`
- Emits: `RetainTerminalRecord`
- To: `Retired`

### `RecoverOpRecordStopped`
- From: `Stopped`
- On: `RecoverOpRecord`(operation_id, status, kind, source, peer_ready, progress_count, terminal_outcome, terminal_payload, completion_sequence)
- Guards:
  - `not_already_registered`
  - `recovered_status_terminal`
  - `terminal_outcome_present`
  - `terminal_payload_present`
  - `completion_sequence_present`
  - `operation_source_valid`
  - `completion_sequence_unclaimed`
  - `terminal_outcome_matches_status`
- Emits: `RetainTerminalRecord`
- To: `Stopped`

### `ClassifyOperationTerminalityTerminalIdle`
- From: `Idle`
- On: `ClassifyOperationTerminality`(operation_id, status)
- Guards:
  - `status_terminal`
- Emits: `OperationTerminal`
- To: `Idle`

### `ClassifyOperationTerminalityNonTerminalIdle`
- From: `Idle`
- On: `ClassifyOperationTerminality`(operation_id, status)
- Guards:
  - `status_non_terminal`
- Emits: `OperationNonTerminal`
- To: `Idle`

### `ClassifyOperationPublicResultMissingAuthorityIdle`
- From: `Idle`
- On: `ClassifyOperationPublicResult`(operation_id, status)
- Guards:
  - `status_missing_authority`
- Emits: `OperationPublicResultClassified`
- To: `Idle`

### `ClassifyOperationPublicResultRunningIdle`
- From: `Idle`
- On: `ClassifyOperationPublicResult`(operation_id, status)
- Guards:
  - `status_running`
- Emits: `OperationPublicResultClassified`
- To: `Idle`

### `ClassifyOperationPublicResultCompletedIdle`
- From: `Idle`
- On: `ClassifyOperationPublicResult`(operation_id, status)
- Guards:
  - `status_completed`
- Emits: `OperationPublicResultClassified`
- To: `Idle`

### `ClassifyOperationPublicResultFailedIdle`
- From: `Idle`
- On: `ClassifyOperationPublicResult`(operation_id, status)
- Guards:
  - `status_failed`
- Emits: `OperationPublicResultClassified`
- To: `Idle`

### `ClassifyOperationPublicResultCancelledIdle`
- From: `Idle`
- On: `ClassifyOperationPublicResult`(operation_id, status)
- Guards:
  - `status_cancelled`
- Emits: `OperationPublicResultClassified`
- To: `Idle`

### `ClassifyOperationTransitionIdempotentSuccessIdle`
- From: `Idle`
- On: `ClassifyOperationTransitionIdempotence`(operation_id, action, status)
- Guards:
  - `transition_idempotent`
- Emits: `OperationTransitionIdempotentSuccess`
- To: `Idle`

### `ClassifyOperationTransitionNotIdempotentIdle`
- From: `Idle`
- On: `ClassifyOperationTransitionIdempotence`(operation_id, action, status)
- Guards:
  - `transition_not_idempotent`
- Emits: `OperationTransitionNotIdempotent`
- To: `Idle`

### `ClassifyOperationCompletionFeedSuppressIdle`
- From: `Idle`
- On: `ClassifyOperationCompletionFeed`(operation_id, kind)
- Guards:
  - `capacity_slot`
- Emits: `OperationCompletionFeedClassified`
- To: `Idle`

### `ClassifyOperationCompletionFeedEmitIdle`
- From: `Idle`
- On: `ClassifyOperationCompletionFeed`(operation_id, kind)
- Guards:
  - `public_completion_kind`
- Emits: `OperationCompletionFeedClassified`
- To: `Idle`

### `ClassifyOperationCompletionWakeWakeIdle`
- From: `Idle`
- On: `ClassifyOperationCompletionWake`(operation_id, kind)
- Guards:
  - `background_tool_completion`
- Emits: `OperationCompletionWakeClassified`
- To: `Idle`

### `ClassifyOperationCompletionWakeIgnoreIdle`
- From: `Idle`
- On: `ClassifyOperationCompletionWake`(operation_id, kind)
- Guards:
  - `non_wake_completion`
- Emits: `OperationCompletionWakeClassified`
- To: `Idle`

### `ClassifyOperationDurabilityDiscardIdle`
- From: `Idle`
- On: `ClassifyOperationDurability`(operation_id, kind)
- Guards:
  - `capacity_slot`
- Emits: `OperationDurabilityClassified`
- To: `Idle`

### `ClassifyOperationDurabilityRetainIdle`
- From: `Idle`
- On: `ClassifyOperationDurability`(operation_id, kind)
- Guards:
  - `durable_kind`
- Emits: `OperationDurabilityClassified`
- To: `Idle`

### `ClassifyRecoveredOperationRecordCapacitySlotDiscardIdle`
- From: `Idle`
- On: `ClassifyRecoveredOperationRecord`(operation_id, status, kind, terminal_outcome_present, terminal_payload_present, completion_sequence_present)
- Guards:
  - `capacity_slot`
- Emits: `DiscardRecoveredOperationRecord`
- To: `Idle`

### `ClassifyRecoveredOperationRecordRetainIdle`
- From: `Idle`
- On: `ClassifyRecoveredOperationRecord`(operation_id, status, kind, terminal_outcome_present, terminal_payload_present, completion_sequence_present)
- Guards:
  - `durable_kind`
  - `status_terminal`
  - `terminal_witnesses_present`
- Emits: `RetainTerminalRecord`
- To: `Idle`

### `ClassifyRecoveredOperationRecordDiscardIdle`
- From: `Idle`
- On: `ClassifyRecoveredOperationRecord`(operation_id, status, kind, terminal_outcome_present, terminal_payload_present, completion_sequence_present)
- Guards:
  - `status_non_terminal`
  - `no_terminal_witnesses`
- Emits: `DiscardRecoveredOperationRecord`
- To: `Idle`

### `RecoverCompletionFeedEntryIdle`
- From: `Idle`
- On: `RecoverCompletionFeedEntry`(operation_id, kind, terminal_outcome, terminal_payload, completion_sequence)
- Guards:
  - `public_completion_kind`
  - `completion_sequence_nonzero`
  - `completion_sequence_within_recovered_cursor`
  - `completion_sequence_unclaimed`
  - `feed_entry_absent`
- Emits: `CompletionFeedEntryRecovered`
- To: `Idle`

### `RecoverCompletionFeedEntryAttached`
- From: `Attached`
- On: `RecoverCompletionFeedEntry`(operation_id, kind, terminal_outcome, terminal_payload, completion_sequence)
- Guards:
  - `public_completion_kind`
  - `completion_sequence_nonzero`
  - `completion_sequence_within_recovered_cursor`
  - `completion_sequence_unclaimed`
  - `feed_entry_absent`
- Emits: `CompletionFeedEntryRecovered`
- To: `Attached`

### `RecoverCompletionFeedEntryRunning`
- From: `Running`
- On: `RecoverCompletionFeedEntry`(operation_id, kind, terminal_outcome, terminal_payload, completion_sequence)
- Guards:
  - `public_completion_kind`
  - `completion_sequence_nonzero`
  - `completion_sequence_within_recovered_cursor`
  - `completion_sequence_unclaimed`
  - `feed_entry_absent`
- Emits: `CompletionFeedEntryRecovered`
- To: `Running`

### `RecoverCompletionFeedEntryRetired`
- From: `Retired`
- On: `RecoverCompletionFeedEntry`(operation_id, kind, terminal_outcome, terminal_payload, completion_sequence)
- Guards:
  - `public_completion_kind`
  - `completion_sequence_nonzero`
  - `completion_sequence_within_recovered_cursor`
  - `completion_sequence_unclaimed`
  - `feed_entry_absent`
- Emits: `CompletionFeedEntryRecovered`
- To: `Retired`

### `RecoverCompletionFeedEntryStopped`
- From: `Stopped`
- On: `RecoverCompletionFeedEntry`(operation_id, kind, terminal_outcome, terminal_payload, completion_sequence)
- Guards:
  - `public_completion_kind`
  - `completion_sequence_nonzero`
  - `completion_sequence_within_recovered_cursor`
  - `completion_sequence_unclaimed`
  - `feed_entry_absent`
- Emits: `CompletionFeedEntryRecovered`
- To: `Stopped`

### `RecoverOpsCompletionCursorIdle`
- From: `Idle`
- On: `RecoverOpsCompletionCursor`(next_completion_seq)
- To: `Idle`

### `RecoverOpsCompletionCursorAttached`
- From: `Attached`
- On: `RecoverOpsCompletionCursor`(next_completion_seq)
- To: `Attached`

### `RecoverOpsCompletionCursorRunning`
- From: `Running`
- On: `RecoverOpsCompletionCursor`(next_completion_seq)
- To: `Running`

### `RecoverOpsCompletionCursorRetired`
- From: `Retired`
- On: `RecoverOpsCompletionCursor`(next_completion_seq)
- To: `Retired`

### `RecoverOpsCompletionCursorStopped`
- From: `Stopped`
- On: `RecoverOpsCompletionCursor`(next_completion_seq)
- To: `Stopped`

### `RecoverCompletionConsumerCursorsIdle`
- From: `Idle`
- On: `RecoverCompletionConsumerCursors`(agent_applied_cursor, runtime_observed_cursor, runtime_injected_cursor)
- Emits: `AgentCompletionCursorAdvanced`, `RuntimeObservedCompletionCursorAdvanced`, `RuntimeInjectedCompletionCursorAdvanced`
- To: `Idle`

### `RecoverCompletionConsumerCursorsAttached`
- From: `Attached`
- On: `RecoverCompletionConsumerCursors`(agent_applied_cursor, runtime_observed_cursor, runtime_injected_cursor)
- Emits: `AgentCompletionCursorAdvanced`, `RuntimeObservedCompletionCursorAdvanced`, `RuntimeInjectedCompletionCursorAdvanced`
- To: `Attached`

### `RecoverCompletionConsumerCursorsRunning`
- From: `Running`
- On: `RecoverCompletionConsumerCursors`(agent_applied_cursor, runtime_observed_cursor, runtime_injected_cursor)
- Emits: `AgentCompletionCursorAdvanced`, `RuntimeObservedCompletionCursorAdvanced`, `RuntimeInjectedCompletionCursorAdvanced`
- To: `Running`

### `RecoverCompletionConsumerCursorsRetired`
- From: `Retired`
- On: `RecoverCompletionConsumerCursors`(agent_applied_cursor, runtime_observed_cursor, runtime_injected_cursor)
- Emits: `AgentCompletionCursorAdvanced`, `RuntimeObservedCompletionCursorAdvanced`, `RuntimeInjectedCompletionCursorAdvanced`
- To: `Retired`

### `RecoverCompletionConsumerCursorsStopped`
- From: `Stopped`
- On: `RecoverCompletionConsumerCursors`(agent_applied_cursor, runtime_observed_cursor, runtime_injected_cursor)
- Emits: `AgentCompletionCursorAdvanced`, `RuntimeObservedCompletionCursorAdvanced`, `RuntimeInjectedCompletionCursorAdvanced`
- To: `Stopped`

### `AdvanceAgentCompletionCursorIdle`
- From: `Idle`
- On: `AdvanceAgentCompletionCursor`(cursor)
- Emits: `AgentCompletionCursorAdvanced`
- To: `Idle`

### `AdvanceAgentCompletionCursorAttached`
- From: `Attached`
- On: `AdvanceAgentCompletionCursor`(cursor)
- Emits: `AgentCompletionCursorAdvanced`
- To: `Attached`

### `AdvanceAgentCompletionCursorRunning`
- From: `Running`
- On: `AdvanceAgentCompletionCursor`(cursor)
- Emits: `AgentCompletionCursorAdvanced`
- To: `Running`

### `AdvanceAgentCompletionCursorRetired`
- From: `Retired`
- On: `AdvanceAgentCompletionCursor`(cursor)
- Emits: `AgentCompletionCursorAdvanced`
- To: `Retired`

### `AdvanceAgentCompletionCursorStopped`
- From: `Stopped`
- On: `AdvanceAgentCompletionCursor`(cursor)
- Emits: `AgentCompletionCursorAdvanced`
- To: `Stopped`

### `AdvanceRuntimeObservedCompletionCursorIdle`
- From: `Idle`
- On: `AdvanceRuntimeObservedCompletionCursor`(cursor)
- Emits: `RuntimeObservedCompletionCursorAdvanced`
- To: `Idle`

### `AdvanceRuntimeObservedCompletionCursorAttached`
- From: `Attached`
- On: `AdvanceRuntimeObservedCompletionCursor`(cursor)
- Emits: `RuntimeObservedCompletionCursorAdvanced`
- To: `Attached`

### `AdvanceRuntimeObservedCompletionCursorRunning`
- From: `Running`
- On: `AdvanceRuntimeObservedCompletionCursor`(cursor)
- Emits: `RuntimeObservedCompletionCursorAdvanced`
- To: `Running`

### `AdvanceRuntimeObservedCompletionCursorRetired`
- From: `Retired`
- On: `AdvanceRuntimeObservedCompletionCursor`(cursor)
- Emits: `RuntimeObservedCompletionCursorAdvanced`
- To: `Retired`

### `AdvanceRuntimeObservedCompletionCursorStopped`
- From: `Stopped`
- On: `AdvanceRuntimeObservedCompletionCursor`(cursor)
- Emits: `RuntimeObservedCompletionCursorAdvanced`
- To: `Stopped`

### `AdvanceRuntimeInjectedCompletionCursorIdle`
- From: `Idle`
- On: `AdvanceRuntimeInjectedCompletionCursor`(cursor)
- Emits: `RuntimeInjectedCompletionCursorAdvanced`
- To: `Idle`

### `AdvanceRuntimeInjectedCompletionCursorAttached`
- From: `Attached`
- On: `AdvanceRuntimeInjectedCompletionCursor`(cursor)
- Emits: `RuntimeInjectedCompletionCursorAdvanced`
- To: `Attached`

### `AdvanceRuntimeInjectedCompletionCursorRunning`
- From: `Running`
- On: `AdvanceRuntimeInjectedCompletionCursor`(cursor)
- Emits: `RuntimeInjectedCompletionCursorAdvanced`
- To: `Running`

### `AdvanceRuntimeInjectedCompletionCursorRetired`
- From: `Retired`
- On: `AdvanceRuntimeInjectedCompletionCursor`(cursor)
- Emits: `RuntimeInjectedCompletionCursorAdvanced`
- To: `Retired`

### `AdvanceRuntimeInjectedCompletionCursorStopped`
- From: `Stopped`
- On: `AdvanceRuntimeInjectedCompletionCursor`(cursor)
- Emits: `RuntimeInjectedCompletionCursorAdvanced`
- To: `Stopped`

### `EvictCompletedOpIdle`
- From: `Idle`
- On: `EvictCompletedOp`(operation_id)
- Guards:
  - `op_registered`
  - `op_terminal`
- Emits: `EvictCompletedRecord`
- To: `Idle`

### `EvictCompletedOpAttached`
- From: `Attached`
- On: `EvictCompletedOp`(operation_id)
- Guards:
  - `op_registered`
  - `op_terminal`
- Emits: `EvictCompletedRecord`
- To: `Attached`

### `EvictCompletedOpRunning`
- From: `Running`
- On: `EvictCompletedOp`(operation_id)
- Guards:
  - `op_registered`
  - `op_terminal`
- Emits: `EvictCompletedRecord`
- To: `Running`

### `EvictCompletedOpRetired`
- From: `Retired`
- On: `EvictCompletedOp`(operation_id)
- Guards:
  - `op_registered`
  - `op_terminal`
- Emits: `EvictCompletedRecord`
- To: `Retired`

### `EvictCompletedOpStopped`
- From: `Stopped`
- On: `EvictCompletedOp`(operation_id)
- Guards:
  - `op_registered`
  - `op_terminal`
- Emits: `EvictCompletedRecord`
- To: `Stopped`

### `CollectCompletedOpIdle`
- From: `Idle`
- On: `CollectCompletedOp`(operation_id)
- Guards:
  - `op_registered`
  - `op_terminal`
- Emits: `CollectCompletedResult`
- To: `Idle`

### `CollectCompletedOpAttached`
- From: `Attached`
- On: `CollectCompletedOp`(operation_id)
- Guards:
  - `op_registered`
  - `op_terminal`
- Emits: `CollectCompletedResult`
- To: `Attached`

### `CollectCompletedOpRunning`
- From: `Running`
- On: `CollectCompletedOp`(operation_id)
- Guards:
  - `op_registered`
  - `op_terminal`
- Emits: `CollectCompletedResult`
- To: `Running`

### `CollectCompletedOpRetired`
- From: `Retired`
- On: `CollectCompletedOp`(operation_id)
- Guards:
  - `op_registered`
  - `op_terminal`
- Emits: `CollectCompletedResult`
- To: `Retired`

### `CollectCompletedOpStopped`
- From: `Stopped`
- On: `CollectCompletedOp`(operation_id)
- Guards:
  - `op_registered`
  - `op_terminal`
- Emits: `CollectCompletedResult`
- To: `Stopped`

### `AdmitSurfaceRequestAcceptedInitializing`
- From: `Initializing`
- On: `AdmitSurfaceRequest`(request_key, terminal_policy)
- Guards:
  - `request_not_tracked`
- Emits: `SurfaceRequestAdmissionAccepted`
- To: `Initializing`

### `AdmitSurfaceRequestDuplicateInitializing`
- From: `Initializing`
- On: `AdmitSurfaceRequest`(request_key, terminal_policy)
- Guards:
  - `request_already_tracked`
- Emits: `SurfaceRequestAdmissionDuplicate`
- To: `Initializing`

### `ClassifySurfaceRequestTerminalMissingInitializing`
- From: `Initializing`
- On: `ClassifySurfaceRequestTerminal`(request_key, success)
- Guards:
  - `request_not_tracked`
- Emits: `SurfaceRequestNotFound`
- To: `Initializing`

### `ClassifySurfaceRequestTerminalPublishInitializing`
- From: `Initializing`
- On: `ClassifySurfaceRequestTerminal`(request_key, success)
- Guards:
  - `request_tracked`
  - `terminal_success`
  - `publish_policy`
- Emits: `SurfaceRequestTerminalPublish`
- To: `Initializing`

### `ClassifySurfaceRequestTerminalFailedInitializing`
- From: `Initializing`
- On: `ClassifySurfaceRequestTerminal`(request_key, success)
- Guards:
  - `request_tracked`
  - `terminal_failed`
- Emits: `SurfaceRequestTerminalRespondWithoutPublish`
- To: `Initializing`

### `ClassifySurfaceRequestTerminalObservationInitializing`
- From: `Initializing`
- On: `ClassifySurfaceRequestTerminal`(request_key, success)
- Guards:
  - `request_tracked`
  - `terminal_success`
  - `observation_policy`
- Emits: `SurfaceRequestTerminalRespondWithoutPublish`
- To: `Initializing`

### `CancelSurfaceRequestMissingInitializing`
- From: `Initializing`
- On: `CancelSurfaceRequest`(request_key)
- Guards:
  - `request_not_tracked`
- Emits: `SurfaceRequestNotFound`
- To: `Initializing`

### `CancelSurfaceRequestPendingInitializing`
- From: `Initializing`
- On: `CancelSurfaceRequest`(request_key)
- Guards:
  - `request_pending`
- Emits: `SurfaceRequestCancelled`
- To: `Initializing`

### `CancelSurfaceRequestAlreadyCancelledInitializing`
- From: `Initializing`
- On: `CancelSurfaceRequest`(request_key)
- Guards:
  - `request_cancelled`
- Emits: `SurfaceRequestAlreadyCancelled`
- To: `Initializing`

### `CancelSurfaceRequestAlreadyPublishedInitializing`
- From: `Initializing`
- On: `CancelSurfaceRequest`(request_key)
- Guards:
  - `request_published`
- Emits: `SurfaceRequestAlreadyPublished`
- To: `Initializing`

### `CancelSurfaceRequestAlreadyCompletedInitializing`
- From: `Initializing`
- On: `CancelSurfaceRequest`(request_key)
- Guards:
  - `request_completed`
- Emits: `SurfaceRequestAlreadyCompleted`
- To: `Initializing`

### `PublishSurfaceRequestMissingInitializing`
- From: `Initializing`
- On: `PublishSurfaceRequest`(request_key)
- Guards:
  - `request_not_tracked`
- Emits: `SurfaceRequestNotFound`
- To: `Initializing`

### `PublishSurfaceRequestPendingInitializing`
- From: `Initializing`
- On: `PublishSurfaceRequest`(request_key)
- Guards:
  - `request_pending`
- Emits: `SurfaceRequestPublished`
- To: `Initializing`

### `PublishSurfaceRequestAlreadyTerminalInitializing`
- From: `Initializing`
- On: `PublishSurfaceRequest`(request_key)
- Guards:
  - `request_terminal`
- Emits: `SurfaceRequestAlreadyTerminal`
- To: `Initializing`

### `PublishOrCancelSurfaceRequestMissingInitializing`
- From: `Initializing`
- On: `PublishOrCancelSurfaceRequest`(request_key)
- Guards:
  - `request_not_tracked`
- Emits: `SurfaceRequestNotFound`
- To: `Initializing`

### `PublishOrCancelSurfaceRequestPendingInitializing`
- From: `Initializing`
- On: `PublishOrCancelSurfaceRequest`(request_key)
- Guards:
  - `request_pending`
- Emits: `SurfaceRequestPublished`
- To: `Initializing`

### `PublishOrCancelSurfaceRequestCancelledInitializing`
- From: `Initializing`
- On: `PublishOrCancelSurfaceRequest`(request_key)
- Guards:
  - `request_cancelled`
- Emits: `SurfaceRequestCancelledBeforePublish`
- To: `Initializing`

### `PublishOrCancelSurfaceRequestAlreadyTerminalInitializing`
- From: `Initializing`
- On: `PublishOrCancelSurfaceRequest`(request_key)
- Guards:
  - `request_terminal`
- Emits: `SurfaceRequestAlreadyTerminal`
- To: `Initializing`

### `FinishSurfaceRequestUnpublishedMissingInitializing`
- From: `Initializing`
- On: `FinishSurfaceRequestUnpublished`(request_key)
- Guards:
  - `request_not_tracked`
- Emits: `SurfaceRequestCompleted`
- To: `Initializing`

### `FinishSurfaceRequestUnpublishedPendingInitializing`
- From: `Initializing`
- On: `FinishSurfaceRequestUnpublished`(request_key)
- Guards:
  - `request_pending`
- Emits: `SurfaceRequestCompleted`
- To: `Initializing`

### `FinishSurfaceRequestUnpublishedCancelledInitializing`
- From: `Initializing`
- On: `FinishSurfaceRequestUnpublished`(request_key)
- Guards:
  - `request_cancelled`
- Emits: `SurfaceRequestSupersededByCancel`
- To: `Initializing`

### `FinishSurfaceRequestUnpublishedTerminalInitializing`
- From: `Initializing`
- On: `FinishSurfaceRequestUnpublished`(request_key)
- Guards:
  - `request_terminal`
- Emits: `SurfaceRequestCompleted`
- To: `Initializing`

### `ResolveLiveOpenAdmissionAcceptedIdle`
- From: `Idle`
- On: `ResolveLiveOpenAdmission`(session_id, channel_id, llm_identity)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `session_not_active`
  - `channel_not_bound`
- Emits: `LiveOpenAdmissionResolved`
- To: `Idle`

### `ResolveLiveOpenAdmissionAcceptedAttached`
- From: `Attached`
- On: `ResolveLiveOpenAdmission`(session_id, channel_id, llm_identity)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `session_not_active`
  - `channel_not_bound`
- Emits: `LiveOpenAdmissionResolved`
- To: `Attached`

### `ResolveLiveOpenAdmissionAcceptedRunning`
- From: `Running`
- On: `ResolveLiveOpenAdmission`(session_id, channel_id, llm_identity)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `session_not_active`
  - `channel_not_bound`
- Emits: `LiveOpenAdmissionResolved`
- To: `Running`

### `ResolveLiveOpenAdmissionAcceptedRetired`
- From: `Retired`
- On: `ResolveLiveOpenAdmission`(session_id, channel_id, llm_identity)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `session_not_active`
  - `channel_not_bound`
- Emits: `LiveOpenAdmissionResolved`
- To: `Retired`

### `ResolveLiveOpenAdmissionAcceptedStopped`
- From: `Stopped`
- On: `ResolveLiveOpenAdmission`(session_id, channel_id, llm_identity)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `session_not_active`
  - `channel_not_bound`
- Emits: `LiveOpenAdmissionResolved`
- To: `Stopped`

### `ResolveLiveOpenAdmissionSessionAlreadyBoundIdle`
- From: `Idle`
- On: `ResolveLiveOpenAdmission`(session_id, channel_id, llm_identity)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `session_active`
- Emits: `LiveOpenAdmissionResolved`
- To: `Idle`

### `ResolveLiveOpenAdmissionSessionAlreadyBoundAttached`
- From: `Attached`
- On: `ResolveLiveOpenAdmission`(session_id, channel_id, llm_identity)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `session_active`
- Emits: `LiveOpenAdmissionResolved`
- To: `Attached`

### `ResolveLiveOpenAdmissionSessionAlreadyBoundRunning`
- From: `Running`
- On: `ResolveLiveOpenAdmission`(session_id, channel_id, llm_identity)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `session_active`
- Emits: `LiveOpenAdmissionResolved`
- To: `Running`

### `ResolveLiveOpenAdmissionSessionAlreadyBoundRetired`
- From: `Retired`
- On: `ResolveLiveOpenAdmission`(session_id, channel_id, llm_identity)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `session_active`
- Emits: `LiveOpenAdmissionResolved`
- To: `Retired`

### `ResolveLiveOpenAdmissionSessionAlreadyBoundStopped`
- From: `Stopped`
- On: `ResolveLiveOpenAdmission`(session_id, channel_id, llm_identity)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `session_active`
- Emits: `LiveOpenAdmissionResolved`
- To: `Stopped`

### `ResolveLiveOpenAdmissionChannelAlreadyBoundIdle`
- From: `Idle`
- On: `ResolveLiveOpenAdmission`(session_id, channel_id, llm_identity)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `session_not_active`
  - `channel_bound`
- Emits: `LiveOpenAdmissionResolved`
- To: `Idle`

### `ResolveLiveOpenAdmissionChannelAlreadyBoundAttached`
- From: `Attached`
- On: `ResolveLiveOpenAdmission`(session_id, channel_id, llm_identity)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `session_not_active`
  - `channel_bound`
- Emits: `LiveOpenAdmissionResolved`
- To: `Attached`

### `ResolveLiveOpenAdmissionChannelAlreadyBoundRunning`
- From: `Running`
- On: `ResolveLiveOpenAdmission`(session_id, channel_id, llm_identity)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `session_not_active`
  - `channel_bound`
- Emits: `LiveOpenAdmissionResolved`
- To: `Running`

### `ResolveLiveOpenAdmissionChannelAlreadyBoundRetired`
- From: `Retired`
- On: `ResolveLiveOpenAdmission`(session_id, channel_id, llm_identity)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `session_not_active`
  - `channel_bound`
- Emits: `LiveOpenAdmissionResolved`
- To: `Retired`

### `ResolveLiveOpenAdmissionChannelAlreadyBoundStopped`
- From: `Stopped`
- On: `ResolveLiveOpenAdmission`(session_id, channel_id, llm_identity)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `session_not_active`
  - `channel_bound`
- Emits: `LiveOpenAdmissionResolved`
- To: `Stopped`

### `AbandonLiveOpenAdmissionIdle`
- From: `Idle`
- On: `AbandonLiveOpenAdmission`(session_id, channel_id)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `session_binding_matches`
  - `channel_binding_matches`
- Emits: `LiveOpenAdmissionAbandoned`
- To: `Idle`

### `AbandonLiveOpenAdmissionAttached`
- From: `Attached`
- On: `AbandonLiveOpenAdmission`(session_id, channel_id)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `session_binding_matches`
  - `channel_binding_matches`
- Emits: `LiveOpenAdmissionAbandoned`
- To: `Attached`

### `AbandonLiveOpenAdmissionRunning`
- From: `Running`
- On: `AbandonLiveOpenAdmission`(session_id, channel_id)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `session_binding_matches`
  - `channel_binding_matches`
- Emits: `LiveOpenAdmissionAbandoned`
- To: `Running`

### `AbandonLiveOpenAdmissionRetired`
- From: `Retired`
- On: `AbandonLiveOpenAdmission`(session_id, channel_id)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `session_binding_matches`
  - `channel_binding_matches`
- Emits: `LiveOpenAdmissionAbandoned`
- To: `Retired`

### `AbandonLiveOpenAdmissionStopped`
- From: `Stopped`
- On: `AbandonLiveOpenAdmission`(session_id, channel_id)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `session_binding_matches`
  - `channel_binding_matches`
- Emits: `LiveOpenAdmissionAbandoned`
- To: `Stopped`

### `RecordLiveRefreshQueuedIdle`
- From: `Idle`
- On: `RecordLiveRefreshQueued`(channel_id, queue_acceptance_sequence)
- Guards:
  - `channel_id_present`
  - `queue_acceptance_sequence_present`
  - `queue_acceptance_sequence_advances`
- Emits: `LiveRefreshResultResolved`
- To: `Idle`

### `RecordLiveRefreshQueuedAttached`
- From: `Attached`
- On: `RecordLiveRefreshQueued`(channel_id, queue_acceptance_sequence)
- Guards:
  - `channel_id_present`
  - `queue_acceptance_sequence_present`
  - `queue_acceptance_sequence_advances`
- Emits: `LiveRefreshResultResolved`
- To: `Attached`

### `RecordLiveRefreshQueuedRunning`
- From: `Running`
- On: `RecordLiveRefreshQueued`(channel_id, queue_acceptance_sequence)
- Guards:
  - `channel_id_present`
  - `queue_acceptance_sequence_present`
  - `queue_acceptance_sequence_advances`
- Emits: `LiveRefreshResultResolved`
- To: `Running`

### `RecordLiveRefreshQueuedRetired`
- From: `Retired`
- On: `RecordLiveRefreshQueued`(channel_id, queue_acceptance_sequence)
- Guards:
  - `channel_id_present`
  - `queue_acceptance_sequence_present`
  - `queue_acceptance_sequence_advances`
- Emits: `LiveRefreshResultResolved`
- To: `Retired`

### `RecordLiveRefreshQueuedStopped`
- From: `Stopped`
- On: `RecordLiveRefreshQueued`(channel_id, queue_acceptance_sequence)
- Guards:
  - `channel_id_present`
  - `queue_acceptance_sequence_present`
  - `queue_acceptance_sequence_advances`
- Emits: `LiveRefreshResultResolved`
- To: `Stopped`

### `RecordLiveCloseClosedIdle`
- From: `Idle`
- On: `RecordLiveCloseClosed`(session_id, channel_id, close_observation_sequence)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `close_observation_sequence_present`
  - `session_binding_matches_if_present`
  - `channel_binding_matches_if_present`
  - `close_observation_sequence_advances`
- Emits: `LiveCloseResultResolved`
- To: `Idle`

### `RecordLiveCloseClosedAttached`
- From: `Attached`
- On: `RecordLiveCloseClosed`(session_id, channel_id, close_observation_sequence)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `close_observation_sequence_present`
  - `session_binding_matches_if_present`
  - `channel_binding_matches_if_present`
  - `close_observation_sequence_advances`
- Emits: `LiveCloseResultResolved`
- To: `Attached`

### `RecordLiveCloseClosedRunning`
- From: `Running`
- On: `RecordLiveCloseClosed`(session_id, channel_id, close_observation_sequence)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `close_observation_sequence_present`
  - `session_binding_matches_if_present`
  - `channel_binding_matches_if_present`
  - `close_observation_sequence_advances`
- Emits: `LiveCloseResultResolved`
- To: `Running`

### `RecordLiveCloseClosedRetired`
- From: `Retired`
- On: `RecordLiveCloseClosed`(session_id, channel_id, close_observation_sequence)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `close_observation_sequence_present`
  - `session_binding_matches_if_present`
  - `channel_binding_matches_if_present`
  - `close_observation_sequence_advances`
- Emits: `LiveCloseResultResolved`
- To: `Retired`

### `RecordLiveCloseClosedStopped`
- From: `Stopped`
- On: `RecordLiveCloseClosed`(session_id, channel_id, close_observation_sequence)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `close_observation_sequence_present`
  - `session_binding_matches_if_present`
  - `channel_binding_matches_if_present`
  - `close_observation_sequence_advances`
- Emits: `LiveCloseResultResolved`
- To: `Stopped`

### `RecordLiveCommandAcceptedIdle`
- From: `Idle`
- On: `RecordLiveCommandAccepted`(channel_id, command, command_acceptance_sequence)
- Guards:
  - `channel_id_present`
  - `command_acceptance_sequence_present`
  - `command_acceptance_sequence_advances`
- Emits: `LiveCommandResultResolved`
- To: `Idle`

### `RecordLiveCommandAcceptedAttached`
- From: `Attached`
- On: `RecordLiveCommandAccepted`(channel_id, command, command_acceptance_sequence)
- Guards:
  - `channel_id_present`
  - `command_acceptance_sequence_present`
  - `command_acceptance_sequence_advances`
- Emits: `LiveCommandResultResolved`
- To: `Attached`

### `RecordLiveCommandAcceptedRunning`
- From: `Running`
- On: `RecordLiveCommandAccepted`(channel_id, command, command_acceptance_sequence)
- Guards:
  - `channel_id_present`
  - `command_acceptance_sequence_present`
  - `command_acceptance_sequence_advances`
- Emits: `LiveCommandResultResolved`
- To: `Running`

### `RecordLiveCommandAcceptedRetired`
- From: `Retired`
- On: `RecordLiveCommandAccepted`(channel_id, command, command_acceptance_sequence)
- Guards:
  - `channel_id_present`
  - `command_acceptance_sequence_present`
  - `command_acceptance_sequence_advances`
- Emits: `LiveCommandResultResolved`
- To: `Retired`

### `RecordLiveCommandAcceptedStopped`
- From: `Stopped`
- On: `RecordLiveCommandAccepted`(channel_id, command, command_acceptance_sequence)
- Guards:
  - `channel_id_present`
  - `command_acceptance_sequence_present`
  - `command_acceptance_sequence_advances`
- Emits: `LiveCommandResultResolved`
- To: `Stopped`

### `RecordLiveCommandRejectedIdle`
- From: `Idle`
- On: `RecordLiveCommandRejected`(channel_id, command, rejection)
- Guards:
  - `channel_id_present`
- Emits: `LiveCommandRejectionResolved`
- To: `Idle`

### `RecordLiveCommandRejectedAttached`
- From: `Attached`
- On: `RecordLiveCommandRejected`(channel_id, command, rejection)
- Guards:
  - `channel_id_present`
- Emits: `LiveCommandRejectionResolved`
- To: `Attached`

### `RecordLiveCommandRejectedRunning`
- From: `Running`
- On: `RecordLiveCommandRejected`(channel_id, command, rejection)
- Guards:
  - `channel_id_present`
- Emits: `LiveCommandRejectionResolved`
- To: `Running`

### `RecordLiveCommandRejectedRetired`
- From: `Retired`
- On: `RecordLiveCommandRejected`(channel_id, command, rejection)
- Guards:
  - `channel_id_present`
- Emits: `LiveCommandRejectionResolved`
- To: `Retired`

### `RecordLiveCommandRejectedStopped`
- From: `Stopped`
- On: `RecordLiveCommandRejected`(channel_id, command, rejection)
- Guards:
  - `channel_id_present`
- Emits: `LiveCommandRejectionResolved`
- To: `Stopped`

### `RecordLiveChannelRequestRejectedIdle`
- From: `Idle`
- On: `RecordLiveChannelRequestRejected`(channel_id, request, rejection)
- Guards:
  - `channel_id_present`
- Emits: `LiveChannelRequestRejectionResolved`
- To: `Idle`

### `RecordLiveChannelRequestRejectedAttached`
- From: `Attached`
- On: `RecordLiveChannelRequestRejected`(channel_id, request, rejection)
- Guards:
  - `channel_id_present`
- Emits: `LiveChannelRequestRejectionResolved`
- To: `Attached`

### `RecordLiveChannelRequestRejectedRunning`
- From: `Running`
- On: `RecordLiveChannelRequestRejected`(channel_id, request, rejection)
- Guards:
  - `channel_id_present`
- Emits: `LiveChannelRequestRejectionResolved`
- To: `Running`

### `RecordLiveChannelRequestRejectedRetired`
- From: `Retired`
- On: `RecordLiveChannelRequestRejected`(channel_id, request, rejection)
- Guards:
  - `channel_id_present`
- Emits: `LiveChannelRequestRejectionResolved`
- To: `Retired`

### `RecordLiveChannelRequestRejectedStopped`
- From: `Stopped`
- On: `RecordLiveChannelRequestRejected`(channel_id, request, rejection)
- Guards:
  - `channel_id_present`
- Emits: `LiveChannelRequestRejectionResolved`
- To: `Stopped`

### `RecordLiveWebrtcTokenIssuedIdle`
- From: `Idle`
- On: `RecordLiveWebrtcTokenIssued`(session_id, channel_id, token, issued_at_ms, ttl_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `issued_at_present`
  - `ttl_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_not_issued`
- Emits: `LiveWebrtcTokenIssued`
- To: `Idle`

### `RecordLiveWebrtcTokenIssuedAttached`
- From: `Attached`
- On: `RecordLiveWebrtcTokenIssued`(session_id, channel_id, token, issued_at_ms, ttl_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `issued_at_present`
  - `ttl_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_not_issued`
- Emits: `LiveWebrtcTokenIssued`
- To: `Attached`

### `RecordLiveWebrtcTokenIssuedRunning`
- From: `Running`
- On: `RecordLiveWebrtcTokenIssued`(session_id, channel_id, token, issued_at_ms, ttl_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `issued_at_present`
  - `ttl_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_not_issued`
- Emits: `LiveWebrtcTokenIssued`
- To: `Running`

### `RecordLiveWebrtcTokenIssuedRetired`
- From: `Retired`
- On: `RecordLiveWebrtcTokenIssued`(session_id, channel_id, token, issued_at_ms, ttl_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `issued_at_present`
  - `ttl_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_not_issued`
- Emits: `LiveWebrtcTokenIssued`
- To: `Retired`

### `RecordLiveWebrtcTokenIssuedStopped`
- From: `Stopped`
- On: `RecordLiveWebrtcTokenIssued`(session_id, channel_id, token, issued_at_ms, ttl_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `issued_at_present`
  - `ttl_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_not_issued`
- Emits: `LiveWebrtcTokenIssued`
- To: `Stopped`

### `ResolveLiveWebrtcAnswerAdmissionAcceptedIdle`
- From: `Idle`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_matches`
  - `token_not_expired`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Idle`

### `ResolveLiveWebrtcAnswerAdmissionAcceptedAttached`
- From: `Attached`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_matches`
  - `token_not_expired`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Attached`

### `ResolveLiveWebrtcAnswerAdmissionAcceptedRunning`
- From: `Running`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_matches`
  - `token_not_expired`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Running`

### `ResolveLiveWebrtcAnswerAdmissionAcceptedRetired`
- From: `Retired`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_matches`
  - `token_not_expired`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Retired`

### `ResolveLiveWebrtcAnswerAdmissionAcceptedStopped`
- From: `Stopped`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_matches`
  - `token_not_expired`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Stopped`

### `ResolveLiveWebrtcAnswerAdmissionChannelNotBoundIdle`
- From: `Idle`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `observed_at_present`
  - `binding_missing_or_mismatch`
  - `no_token_state_rejection_pending`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Idle`

### `ResolveLiveWebrtcAnswerAdmissionChannelNotBoundAttached`
- From: `Attached`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `observed_at_present`
  - `binding_missing_or_mismatch`
  - `no_token_state_rejection_pending`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Attached`

### `ResolveLiveWebrtcAnswerAdmissionChannelNotBoundRunning`
- From: `Running`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `observed_at_present`
  - `binding_missing_or_mismatch`
  - `no_token_state_rejection_pending`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Running`

### `ResolveLiveWebrtcAnswerAdmissionChannelNotBoundRetired`
- From: `Retired`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `observed_at_present`
  - `binding_missing_or_mismatch`
  - `no_token_state_rejection_pending`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Retired`

### `ResolveLiveWebrtcAnswerAdmissionChannelNotBoundStopped`
- From: `Stopped`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `observed_at_present`
  - `binding_missing_or_mismatch`
  - `no_token_state_rejection_pending`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Stopped`

### `ResolveLiveWebrtcAnswerAdmissionTokenNotFoundIdle`
- From: `Idle`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `observed_at_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_unknown`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Idle`

### `ResolveLiveWebrtcAnswerAdmissionTokenNotFoundAttached`
- From: `Attached`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `observed_at_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_unknown`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Attached`

### `ResolveLiveWebrtcAnswerAdmissionTokenNotFoundRunning`
- From: `Running`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `observed_at_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_unknown`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Running`

### `ResolveLiveWebrtcAnswerAdmissionTokenNotFoundRetired`
- From: `Retired`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `observed_at_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_unknown`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Retired`

### `ResolveLiveWebrtcAnswerAdmissionTokenNotFoundStopped`
- From: `Stopped`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `observed_at_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_unknown`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Stopped`

### `ResolveLiveWebrtcAnswerAdmissionTokenAlreadyConsumedIdle`
- From: `Idle`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_consumed`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Idle`

### `ResolveLiveWebrtcAnswerAdmissionTokenAlreadyConsumedAttached`
- From: `Attached`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_consumed`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Attached`

### `ResolveLiveWebrtcAnswerAdmissionTokenAlreadyConsumedRunning`
- From: `Running`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_consumed`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Running`

### `ResolveLiveWebrtcAnswerAdmissionTokenAlreadyConsumedRetired`
- From: `Retired`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_consumed`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Retired`

### `ResolveLiveWebrtcAnswerAdmissionTokenAlreadyConsumedStopped`
- From: `Stopped`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_consumed`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Stopped`

### `ResolveLiveWebrtcAnswerAdmissionTokenChannelMismatchIdle`
- From: `Idle`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_mismatch`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Idle`

### `ResolveLiveWebrtcAnswerAdmissionTokenChannelMismatchAttached`
- From: `Attached`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_mismatch`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Attached`

### `ResolveLiveWebrtcAnswerAdmissionTokenChannelMismatchRunning`
- From: `Running`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_mismatch`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Running`

### `ResolveLiveWebrtcAnswerAdmissionTokenChannelMismatchRetired`
- From: `Retired`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_mismatch`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Retired`

### `ResolveLiveWebrtcAnswerAdmissionTokenChannelMismatchStopped`
- From: `Stopped`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_mismatch`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Stopped`

### `ResolveLiveWebrtcAnswerAdmissionTokenExpiredIdle`
- From: `Idle`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_matches`
  - `token_expired`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Idle`

### `ResolveLiveWebrtcAnswerAdmissionTokenExpiredAttached`
- From: `Attached`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_matches`
  - `token_expired`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Attached`

### `ResolveLiveWebrtcAnswerAdmissionTokenExpiredRunning`
- From: `Running`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_matches`
  - `token_expired`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Running`

### `ResolveLiveWebrtcAnswerAdmissionTokenExpiredRetired`
- From: `Retired`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_matches`
  - `token_expired`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Retired`

### `ResolveLiveWebrtcAnswerAdmissionTokenExpiredStopped`
- From: `Stopped`
- On: `ResolveLiveWebrtcAnswerAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_matches`
  - `token_expired`
- Emits: `LiveWebrtcAnswerAdmissionResolved`
- To: `Stopped`

### `RecordLiveWebrtcAnswerAcceptedIdle`
- From: `Idle`
- On: `RecordLiveWebrtcAnswerAccepted`(session_id, channel_id, answer_observation_sequence)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `answer_observation_sequence_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `answer_observation_sequence_advances`
- Emits: `LiveWebrtcAnswerResultResolved`
- To: `Idle`

### `RecordLiveWebrtcAnswerAcceptedAttached`
- From: `Attached`
- On: `RecordLiveWebrtcAnswerAccepted`(session_id, channel_id, answer_observation_sequence)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `answer_observation_sequence_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `answer_observation_sequence_advances`
- Emits: `LiveWebrtcAnswerResultResolved`
- To: `Attached`

### `RecordLiveWebrtcAnswerAcceptedRunning`
- From: `Running`
- On: `RecordLiveWebrtcAnswerAccepted`(session_id, channel_id, answer_observation_sequence)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `answer_observation_sequence_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `answer_observation_sequence_advances`
- Emits: `LiveWebrtcAnswerResultResolved`
- To: `Running`

### `RecordLiveWebrtcAnswerAcceptedRetired`
- From: `Retired`
- On: `RecordLiveWebrtcAnswerAccepted`(session_id, channel_id, answer_observation_sequence)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `answer_observation_sequence_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `answer_observation_sequence_advances`
- Emits: `LiveWebrtcAnswerResultResolved`
- To: `Retired`

### `RecordLiveWebrtcAnswerAcceptedStopped`
- From: `Stopped`
- On: `RecordLiveWebrtcAnswerAccepted`(session_id, channel_id, answer_observation_sequence)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `answer_observation_sequence_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `answer_observation_sequence_advances`
- Emits: `LiveWebrtcAnswerResultResolved`
- To: `Stopped`

### `RecordLiveWebsocketTokenIssuedIdle`
- From: `Idle`
- On: `RecordLiveWebsocketTokenIssued`(session_id, channel_id, token, issued_at_ms, ttl_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `issued_at_present`
  - `ttl_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_not_issued`
- Emits: `LiveWebsocketTokenIssued`
- To: `Idle`

### `RecordLiveWebsocketTokenIssuedAttached`
- From: `Attached`
- On: `RecordLiveWebsocketTokenIssued`(session_id, channel_id, token, issued_at_ms, ttl_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `issued_at_present`
  - `ttl_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_not_issued`
- Emits: `LiveWebsocketTokenIssued`
- To: `Attached`

### `RecordLiveWebsocketTokenIssuedRunning`
- From: `Running`
- On: `RecordLiveWebsocketTokenIssued`(session_id, channel_id, token, issued_at_ms, ttl_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `issued_at_present`
  - `ttl_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_not_issued`
- Emits: `LiveWebsocketTokenIssued`
- To: `Running`

### `RecordLiveWebsocketTokenIssuedRetired`
- From: `Retired`
- On: `RecordLiveWebsocketTokenIssued`(session_id, channel_id, token, issued_at_ms, ttl_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `issued_at_present`
  - `ttl_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_not_issued`
- Emits: `LiveWebsocketTokenIssued`
- To: `Retired`

### `RecordLiveWebsocketTokenIssuedStopped`
- From: `Stopped`
- On: `RecordLiveWebsocketTokenIssued`(session_id, channel_id, token, issued_at_ms, ttl_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `issued_at_present`
  - `ttl_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_not_issued`
- Emits: `LiveWebsocketTokenIssued`
- To: `Stopped`

### `ResolveLiveWebsocketTokenAdmissionAcceptedIdle`
- From: `Idle`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_matches`
  - `token_not_expired`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Idle`

### `ResolveLiveWebsocketTokenAdmissionAcceptedAttached`
- From: `Attached`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_matches`
  - `token_not_expired`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Attached`

### `ResolveLiveWebsocketTokenAdmissionAcceptedRunning`
- From: `Running`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_matches`
  - `token_not_expired`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Running`

### `ResolveLiveWebsocketTokenAdmissionAcceptedRetired`
- From: `Retired`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_matches`
  - `token_not_expired`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Retired`

### `ResolveLiveWebsocketTokenAdmissionAcceptedStopped`
- From: `Stopped`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_matches`
  - `token_not_expired`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Stopped`

### `ResolveLiveWebsocketTokenAdmissionChannelNotBoundIdle`
- From: `Idle`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `channel_id_present`
  - `observed_at_present`
  - `binding_missing_or_mismatch`
  - `no_token_state_rejection_pending`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Idle`

### `ResolveLiveWebsocketTokenAdmissionChannelNotBoundAttached`
- From: `Attached`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `channel_id_present`
  - `observed_at_present`
  - `binding_missing_or_mismatch`
  - `no_token_state_rejection_pending`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Attached`

### `ResolveLiveWebsocketTokenAdmissionChannelNotBoundRunning`
- From: `Running`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `channel_id_present`
  - `observed_at_present`
  - `binding_missing_or_mismatch`
  - `no_token_state_rejection_pending`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Running`

### `ResolveLiveWebsocketTokenAdmissionChannelNotBoundRetired`
- From: `Retired`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `channel_id_present`
  - `observed_at_present`
  - `binding_missing_or_mismatch`
  - `no_token_state_rejection_pending`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Retired`

### `ResolveLiveWebsocketTokenAdmissionChannelNotBoundStopped`
- From: `Stopped`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `channel_id_present`
  - `observed_at_present`
  - `binding_missing_or_mismatch`
  - `no_token_state_rejection_pending`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Stopped`

### `ResolveLiveWebsocketTokenAdmissionTokenNotFoundIdle`
- From: `Idle`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `observed_at_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_unknown`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Idle`

### `ResolveLiveWebsocketTokenAdmissionTokenNotFoundAttached`
- From: `Attached`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `observed_at_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_unknown`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Attached`

### `ResolveLiveWebsocketTokenAdmissionTokenNotFoundRunning`
- From: `Running`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `observed_at_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_unknown`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Running`

### `ResolveLiveWebsocketTokenAdmissionTokenNotFoundRetired`
- From: `Retired`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `observed_at_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_unknown`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Retired`

### `ResolveLiveWebsocketTokenAdmissionTokenNotFoundStopped`
- From: `Stopped`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `observed_at_present`
  - `session_binding_matches`
  - `channel_binding_matches`
  - `token_unknown`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Stopped`

### `ResolveLiveWebsocketTokenAdmissionTokenAlreadyConsumedIdle`
- From: `Idle`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_consumed`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Idle`

### `ResolveLiveWebsocketTokenAdmissionTokenAlreadyConsumedAttached`
- From: `Attached`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_consumed`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Attached`

### `ResolveLiveWebsocketTokenAdmissionTokenAlreadyConsumedRunning`
- From: `Running`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_consumed`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Running`

### `ResolveLiveWebsocketTokenAdmissionTokenAlreadyConsumedRetired`
- From: `Retired`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_consumed`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Retired`

### `ResolveLiveWebsocketTokenAdmissionTokenAlreadyConsumedStopped`
- From: `Stopped`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_consumed`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Stopped`

### `ResolveLiveWebsocketTokenAdmissionTokenChannelMismatchIdle`
- From: `Idle`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_mismatch`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Idle`

### `ResolveLiveWebsocketTokenAdmissionTokenChannelMismatchAttached`
- From: `Attached`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_mismatch`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Attached`

### `ResolveLiveWebsocketTokenAdmissionTokenChannelMismatchRunning`
- From: `Running`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_mismatch`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Running`

### `ResolveLiveWebsocketTokenAdmissionTokenChannelMismatchRetired`
- From: `Retired`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_mismatch`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Retired`

### `ResolveLiveWebsocketTokenAdmissionTokenChannelMismatchStopped`
- From: `Stopped`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_mismatch`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Stopped`

### `ResolveLiveWebsocketTokenAdmissionTokenExpiredIdle`
- From: `Idle`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_matches`
  - `token_expired`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Idle`

### `ResolveLiveWebsocketTokenAdmissionTokenExpiredAttached`
- From: `Attached`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_matches`
  - `token_expired`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Attached`

### `ResolveLiveWebsocketTokenAdmissionTokenExpiredRunning`
- From: `Running`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_matches`
  - `token_expired`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Running`

### `ResolveLiveWebsocketTokenAdmissionTokenExpiredRetired`
- From: `Retired`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_matches`
  - `token_expired`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Retired`

### `ResolveLiveWebsocketTokenAdmissionTokenExpiredStopped`
- From: `Stopped`
- On: `ResolveLiveWebsocketTokenAdmission`(session_id, channel_id, token, observed_at_ms)
- Guards:
  - `session_id_present`
  - `channel_id_present`
  - `token_present`
  - `observed_at_present`
  - `token_known`
  - `token_not_consumed`
  - `token_channel_matches`
  - `token_expired`
- Emits: `LiveWebsocketTokenAdmissionResolved`
- To: `Stopped`

### `RecordSessionEventStreamOpenedIdle`
- From: `Idle`
- On: `RecordSessionEventStreamOpened`(stream_id, session_id)
- Guards:
  - `stream_id_present`
  - `session_id_present`
  - `stream_not_active`
  - `stream_not_closed`
- Emits: `SessionEventStreamOpenResolved`
- To: `Idle`

### `RecordSessionEventStreamOpenedAttached`
- From: `Attached`
- On: `RecordSessionEventStreamOpened`(stream_id, session_id)
- Guards:
  - `stream_id_present`
  - `session_id_present`
  - `stream_not_active`
  - `stream_not_closed`
- Emits: `SessionEventStreamOpenResolved`
- To: `Attached`

### `RecordSessionEventStreamOpenedRunning`
- From: `Running`
- On: `RecordSessionEventStreamOpened`(stream_id, session_id)
- Guards:
  - `stream_id_present`
  - `session_id_present`
  - `stream_not_active`
  - `stream_not_closed`
- Emits: `SessionEventStreamOpenResolved`
- To: `Running`

### `RecordSessionEventStreamOpenedRetired`
- From: `Retired`
- On: `RecordSessionEventStreamOpened`(stream_id, session_id)
- Guards:
  - `stream_id_present`
  - `session_id_present`
  - `stream_not_active`
  - `stream_not_closed`
- Emits: `SessionEventStreamOpenResolved`
- To: `Retired`

### `RecordSessionEventStreamOpenedStopped`
- From: `Stopped`
- On: `RecordSessionEventStreamOpened`(stream_id, session_id)
- Guards:
  - `stream_id_present`
  - `session_id_present`
  - `stream_not_active`
  - `stream_not_closed`
- Emits: `SessionEventStreamOpenResolved`
- To: `Stopped`

### `RecordSessionEventStreamTerminatedIdle`
- From: `Idle`
- On: `RecordSessionEventStreamTerminated`(stream_id, observation, detail)
- Guards:
  - `stream_id_present`
  - `stream_active`
  - `session_binding_recorded`
  - `terminal_detail_matches_observation`
- Emits: `SessionEventStreamTerminalResolved`
- To: `Idle`

### `RecordSessionEventStreamTerminatedAttached`
- From: `Attached`
- On: `RecordSessionEventStreamTerminated`(stream_id, observation, detail)
- Guards:
  - `stream_id_present`
  - `stream_active`
  - `session_binding_recorded`
  - `terminal_detail_matches_observation`
- Emits: `SessionEventStreamTerminalResolved`
- To: `Attached`

### `RecordSessionEventStreamTerminatedRunning`
- From: `Running`
- On: `RecordSessionEventStreamTerminated`(stream_id, observation, detail)
- Guards:
  - `stream_id_present`
  - `stream_active`
  - `session_binding_recorded`
  - `terminal_detail_matches_observation`
- Emits: `SessionEventStreamTerminalResolved`
- To: `Running`

### `RecordSessionEventStreamTerminatedRetired`
- From: `Retired`
- On: `RecordSessionEventStreamTerminated`(stream_id, observation, detail)
- Guards:
  - `stream_id_present`
  - `stream_active`
  - `session_binding_recorded`
  - `terminal_detail_matches_observation`
- Emits: `SessionEventStreamTerminalResolved`
- To: `Retired`

### `RecordSessionEventStreamTerminatedStopped`
- From: `Stopped`
- On: `RecordSessionEventStreamTerminated`(stream_id, observation, detail)
- Guards:
  - `stream_id_present`
  - `stream_active`
  - `session_binding_recorded`
  - `terminal_detail_matches_observation`
- Emits: `SessionEventStreamTerminalResolved`
- To: `Stopped`

### `ResolveSessionEventStreamCloseActiveIdle`
- From: `Idle`
- On: `ResolveSessionEventStreamClose`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_active`
  - `session_binding_recorded`
- Emits: `SessionEventStreamCloseResolved`, `SessionEventStreamTerminalResolved`
- To: `Idle`

### `ResolveSessionEventStreamCloseActiveAttached`
- From: `Attached`
- On: `ResolveSessionEventStreamClose`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_active`
  - `session_binding_recorded`
- Emits: `SessionEventStreamCloseResolved`, `SessionEventStreamTerminalResolved`
- To: `Attached`

### `ResolveSessionEventStreamCloseActiveRunning`
- From: `Running`
- On: `ResolveSessionEventStreamClose`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_active`
  - `session_binding_recorded`
- Emits: `SessionEventStreamCloseResolved`, `SessionEventStreamTerminalResolved`
- To: `Running`

### `ResolveSessionEventStreamCloseActiveRetired`
- From: `Retired`
- On: `ResolveSessionEventStreamClose`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_active`
  - `session_binding_recorded`
- Emits: `SessionEventStreamCloseResolved`, `SessionEventStreamTerminalResolved`
- To: `Retired`

### `ResolveSessionEventStreamCloseActiveStopped`
- From: `Stopped`
- On: `ResolveSessionEventStreamClose`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_active`
  - `session_binding_recorded`
- Emits: `SessionEventStreamCloseResolved`, `SessionEventStreamTerminalResolved`
- To: `Stopped`

### `ResolveSessionEventStreamCloseAlreadyClosedIdle`
- From: `Idle`
- On: `ResolveSessionEventStreamClose`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_not_active`
  - `stream_closed`
- Emits: `SessionEventStreamCloseResolved`
- To: `Idle`

### `ResolveSessionEventStreamCloseAlreadyClosedAttached`
- From: `Attached`
- On: `ResolveSessionEventStreamClose`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_not_active`
  - `stream_closed`
- Emits: `SessionEventStreamCloseResolved`
- To: `Attached`

### `ResolveSessionEventStreamCloseAlreadyClosedRunning`
- From: `Running`
- On: `ResolveSessionEventStreamClose`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_not_active`
  - `stream_closed`
- Emits: `SessionEventStreamCloseResolved`
- To: `Running`

### `ResolveSessionEventStreamCloseAlreadyClosedRetired`
- From: `Retired`
- On: `ResolveSessionEventStreamClose`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_not_active`
  - `stream_closed`
- Emits: `SessionEventStreamCloseResolved`
- To: `Retired`

### `ResolveSessionEventStreamCloseAlreadyClosedStopped`
- From: `Stopped`
- On: `ResolveSessionEventStreamClose`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_not_active`
  - `stream_closed`
- Emits: `SessionEventStreamCloseResolved`
- To: `Stopped`

### `RecordMobEventStreamOpenedIdle`
- From: `Idle`
- On: `RecordMobEventStreamOpened`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_not_active`
  - `stream_not_closed`
- Emits: `MobEventStreamOpenResolved`
- To: `Idle`

### `RecordMobEventStreamOpenedAttached`
- From: `Attached`
- On: `RecordMobEventStreamOpened`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_not_active`
  - `stream_not_closed`
- Emits: `MobEventStreamOpenResolved`
- To: `Attached`

### `RecordMobEventStreamOpenedRunning`
- From: `Running`
- On: `RecordMobEventStreamOpened`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_not_active`
  - `stream_not_closed`
- Emits: `MobEventStreamOpenResolved`
- To: `Running`

### `RecordMobEventStreamOpenedRetired`
- From: `Retired`
- On: `RecordMobEventStreamOpened`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_not_active`
  - `stream_not_closed`
- Emits: `MobEventStreamOpenResolved`
- To: `Retired`

### `RecordMobEventStreamOpenedStopped`
- From: `Stopped`
- On: `RecordMobEventStreamOpened`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_not_active`
  - `stream_not_closed`
- Emits: `MobEventStreamOpenResolved`
- To: `Stopped`

### `RecordMobEventStreamTerminatedIdle`
- From: `Idle`
- On: `RecordMobEventStreamTerminated`(stream_id, observation, detail)
- Guards:
  - `stream_id_present`
  - `stream_active`
  - `terminal_detail_matches_observation`
- Emits: `MobEventStreamTerminalResolved`
- To: `Idle`

### `RecordMobEventStreamTerminatedAttached`
- From: `Attached`
- On: `RecordMobEventStreamTerminated`(stream_id, observation, detail)
- Guards:
  - `stream_id_present`
  - `stream_active`
  - `terminal_detail_matches_observation`
- Emits: `MobEventStreamTerminalResolved`
- To: `Attached`

### `RecordMobEventStreamTerminatedRunning`
- From: `Running`
- On: `RecordMobEventStreamTerminated`(stream_id, observation, detail)
- Guards:
  - `stream_id_present`
  - `stream_active`
  - `terminal_detail_matches_observation`
- Emits: `MobEventStreamTerminalResolved`
- To: `Running`

### `RecordMobEventStreamTerminatedRetired`
- From: `Retired`
- On: `RecordMobEventStreamTerminated`(stream_id, observation, detail)
- Guards:
  - `stream_id_present`
  - `stream_active`
  - `terminal_detail_matches_observation`
- Emits: `MobEventStreamTerminalResolved`
- To: `Retired`

### `RecordMobEventStreamTerminatedStopped`
- From: `Stopped`
- On: `RecordMobEventStreamTerminated`(stream_id, observation, detail)
- Guards:
  - `stream_id_present`
  - `stream_active`
  - `terminal_detail_matches_observation`
- Emits: `MobEventStreamTerminalResolved`
- To: `Stopped`

### `ResolveMobEventStreamCloseActiveIdle`
- From: `Idle`
- On: `ResolveMobEventStreamClose`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_active`
- Emits: `MobEventStreamCloseResolved`, `MobEventStreamTerminalResolved`
- To: `Idle`

### `ResolveMobEventStreamCloseActiveAttached`
- From: `Attached`
- On: `ResolveMobEventStreamClose`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_active`
- Emits: `MobEventStreamCloseResolved`, `MobEventStreamTerminalResolved`
- To: `Attached`

### `ResolveMobEventStreamCloseActiveRunning`
- From: `Running`
- On: `ResolveMobEventStreamClose`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_active`
- Emits: `MobEventStreamCloseResolved`, `MobEventStreamTerminalResolved`
- To: `Running`

### `ResolveMobEventStreamCloseActiveRetired`
- From: `Retired`
- On: `ResolveMobEventStreamClose`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_active`
- Emits: `MobEventStreamCloseResolved`, `MobEventStreamTerminalResolved`
- To: `Retired`

### `ResolveMobEventStreamCloseActiveStopped`
- From: `Stopped`
- On: `ResolveMobEventStreamClose`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_active`
- Emits: `MobEventStreamCloseResolved`, `MobEventStreamTerminalResolved`
- To: `Stopped`

### `ResolveMobEventStreamCloseAlreadyClosedIdle`
- From: `Idle`
- On: `ResolveMobEventStreamClose`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_not_active`
  - `stream_closed`
- Emits: `MobEventStreamCloseResolved`
- To: `Idle`

### `ResolveMobEventStreamCloseAlreadyClosedAttached`
- From: `Attached`
- On: `ResolveMobEventStreamClose`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_not_active`
  - `stream_closed`
- Emits: `MobEventStreamCloseResolved`
- To: `Attached`

### `ResolveMobEventStreamCloseAlreadyClosedRunning`
- From: `Running`
- On: `ResolveMobEventStreamClose`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_not_active`
  - `stream_closed`
- Emits: `MobEventStreamCloseResolved`
- To: `Running`

### `ResolveMobEventStreamCloseAlreadyClosedRetired`
- From: `Retired`
- On: `ResolveMobEventStreamClose`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_not_active`
  - `stream_closed`
- Emits: `MobEventStreamCloseResolved`
- To: `Retired`

### `ResolveMobEventStreamCloseAlreadyClosedStopped`
- From: `Stopped`
- On: `ResolveMobEventStreamClose`(stream_id)
- Guards:
  - `stream_id_present`
  - `stream_not_active`
  - `stream_closed`
- Emits: `MobEventStreamCloseResolved`
- To: `Stopped`

### `RecordLiveChannelStatusIdle`
- From: `Idle`
- On: `RecordLiveChannelStatus`(channel_id, status, status_observation_sequence, degradation_reason, degradation_detail)
- Guards:
  - `channel_id_present`
  - `status_observation_sequence_present`
  - `status_observation_sequence_advances`
  - `degradation_fields_match_status`
- Emits: `LiveChannelStatusResolved`
- To: `Idle`

### `RecordLiveChannelStatusAttached`
- From: `Attached`
- On: `RecordLiveChannelStatus`(channel_id, status, status_observation_sequence, degradation_reason, degradation_detail)
- Guards:
  - `channel_id_present`
  - `status_observation_sequence_present`
  - `status_observation_sequence_advances`
  - `degradation_fields_match_status`
- Emits: `LiveChannelStatusResolved`
- To: `Attached`

### `RecordLiveChannelStatusRunning`
- From: `Running`
- On: `RecordLiveChannelStatus`(channel_id, status, status_observation_sequence, degradation_reason, degradation_detail)
- Guards:
  - `channel_id_present`
  - `status_observation_sequence_present`
  - `status_observation_sequence_advances`
  - `degradation_fields_match_status`
- Emits: `LiveChannelStatusResolved`
- To: `Running`

### `RecordLiveChannelStatusRetired`
- From: `Retired`
- On: `RecordLiveChannelStatus`(channel_id, status, status_observation_sequence, degradation_reason, degradation_detail)
- Guards:
  - `channel_id_present`
  - `status_observation_sequence_present`
  - `status_observation_sequence_advances`
  - `degradation_fields_match_status`
- Emits: `LiveChannelStatusResolved`
- To: `Retired`

### `RecordLiveChannelStatusStopped`
- From: `Stopped`
- On: `RecordLiveChannelStatus`(channel_id, status, status_observation_sequence, degradation_reason, degradation_detail)
- Guards:
  - `channel_id_present`
  - `status_observation_sequence_present`
  - `status_observation_sequence_advances`
  - `degradation_fields_match_status`
- Emits: `LiveChannelStatusResolved`
- To: `Stopped`

### `ResolveWaitAllAdmissionDuplicateRejectedIdle`
- From: `Idle`
- On: `ResolveWaitAllAdmission`(wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token, duplicate_operation_id, not_found_operation_id)
- Guards:
  - `duplicate_observed`
  - `duplicate_witness_present`
  - `duplicate_witness_requested`
  - `duplicate_witness_repeated`
- Emits: `WaitAllAdmissionResolved`
- To: `Idle`

### `ResolveWaitAllAdmissionDuplicateRejectedAttached`
- From: `Attached`
- On: `ResolveWaitAllAdmission`(wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token, duplicate_operation_id, not_found_operation_id)
- Guards:
  - `duplicate_observed`
  - `duplicate_witness_present`
  - `duplicate_witness_requested`
  - `duplicate_witness_repeated`
- Emits: `WaitAllAdmissionResolved`
- To: `Attached`

### `ResolveWaitAllAdmissionDuplicateRejectedRunning`
- From: `Running`
- On: `ResolveWaitAllAdmission`(wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token, duplicate_operation_id, not_found_operation_id)
- Guards:
  - `duplicate_observed`
  - `duplicate_witness_present`
  - `duplicate_witness_requested`
  - `duplicate_witness_repeated`
- Emits: `WaitAllAdmissionResolved`
- To: `Running`

### `ResolveWaitAllAdmissionDuplicateRejectedRetired`
- From: `Retired`
- On: `ResolveWaitAllAdmission`(wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token, duplicate_operation_id, not_found_operation_id)
- Guards:
  - `duplicate_observed`
  - `duplicate_witness_present`
  - `duplicate_witness_requested`
  - `duplicate_witness_repeated`
- Emits: `WaitAllAdmissionResolved`
- To: `Retired`

### `ResolveWaitAllAdmissionDuplicateRejectedStopped`
- From: `Stopped`
- On: `ResolveWaitAllAdmission`(wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token, duplicate_operation_id, not_found_operation_id)
- Guards:
  - `duplicate_observed`
  - `duplicate_witness_present`
  - `duplicate_witness_requested`
  - `duplicate_witness_repeated`
- Emits: `WaitAllAdmissionResolved`
- To: `Stopped`

### `ResolveWaitAllAdmissionActiveRejectedIdle`
- From: `Idle`
- On: `ResolveWaitAllAdmission`(wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token, duplicate_operation_id, not_found_operation_id)
- Guards:
  - `no_duplicate_observed`
  - `wait_already_active`
- Emits: `WaitAllAdmissionResolved`
- To: `Idle`

### `ResolveWaitAllAdmissionActiveRejectedAttached`
- From: `Attached`
- On: `ResolveWaitAllAdmission`(wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token, duplicate_operation_id, not_found_operation_id)
- Guards:
  - `no_duplicate_observed`
  - `wait_already_active`
- Emits: `WaitAllAdmissionResolved`
- To: `Attached`

### `ResolveWaitAllAdmissionActiveRejectedRunning`
- From: `Running`
- On: `ResolveWaitAllAdmission`(wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token, duplicate_operation_id, not_found_operation_id)
- Guards:
  - `no_duplicate_observed`
  - `wait_already_active`
- Emits: `WaitAllAdmissionResolved`
- To: `Running`

### `ResolveWaitAllAdmissionActiveRejectedRetired`
- From: `Retired`
- On: `ResolveWaitAllAdmission`(wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token, duplicate_operation_id, not_found_operation_id)
- Guards:
  - `no_duplicate_observed`
  - `wait_already_active`
- Emits: `WaitAllAdmissionResolved`
- To: `Retired`

### `ResolveWaitAllAdmissionActiveRejectedStopped`
- From: `Stopped`
- On: `ResolveWaitAllAdmission`(wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token, duplicate_operation_id, not_found_operation_id)
- Guards:
  - `no_duplicate_observed`
  - `wait_already_active`
- Emits: `WaitAllAdmissionResolved`
- To: `Stopped`

### `ResolveWaitAllAdmissionNotFoundRejectedIdle`
- From: `Idle`
- On: `ResolveWaitAllAdmission`(wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token, duplicate_operation_id, not_found_operation_id)
- Guards:
  - `no_duplicate_observed`
  - `wait_inactive`
  - `operation_missing`
  - `not_found_witness_present`
  - `not_found_witness_requested`
  - `not_found_witness_missing`
- Emits: `WaitAllAdmissionResolved`
- To: `Idle`

### `ResolveWaitAllAdmissionNotFoundRejectedAttached`
- From: `Attached`
- On: `ResolveWaitAllAdmission`(wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token, duplicate_operation_id, not_found_operation_id)
- Guards:
  - `no_duplicate_observed`
  - `wait_inactive`
  - `operation_missing`
  - `not_found_witness_present`
  - `not_found_witness_requested`
  - `not_found_witness_missing`
- Emits: `WaitAllAdmissionResolved`
- To: `Attached`

### `ResolveWaitAllAdmissionNotFoundRejectedRunning`
- From: `Running`
- On: `ResolveWaitAllAdmission`(wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token, duplicate_operation_id, not_found_operation_id)
- Guards:
  - `no_duplicate_observed`
  - `wait_inactive`
  - `operation_missing`
  - `not_found_witness_present`
  - `not_found_witness_requested`
  - `not_found_witness_missing`
- Emits: `WaitAllAdmissionResolved`
- To: `Running`

### `ResolveWaitAllAdmissionNotFoundRejectedRetired`
- From: `Retired`
- On: `ResolveWaitAllAdmission`(wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token, duplicate_operation_id, not_found_operation_id)
- Guards:
  - `no_duplicate_observed`
  - `wait_inactive`
  - `operation_missing`
  - `not_found_witness_present`
  - `not_found_witness_requested`
  - `not_found_witness_missing`
- Emits: `WaitAllAdmissionResolved`
- To: `Retired`

### `ResolveWaitAllAdmissionNotFoundRejectedStopped`
- From: `Stopped`
- On: `ResolveWaitAllAdmission`(wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token, duplicate_operation_id, not_found_operation_id)
- Guards:
  - `no_duplicate_observed`
  - `wait_inactive`
  - `operation_missing`
  - `not_found_witness_present`
  - `not_found_witness_requested`
  - `not_found_witness_missing`
- Emits: `WaitAllAdmissionResolved`
- To: `Stopped`

### `ResolveWaitAllAdmissionAcceptedIdle`
- From: `Idle`
- On: `ResolveWaitAllAdmission`(wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token, duplicate_operation_id, not_found_operation_id)
- Guards:
  - `no_duplicate_observed`
  - `wait_inactive`
  - `operations_tracked`
  - `operation_token_witness_valid`
- Emits: `WaitAllAdmissionResolved`
- To: `Idle`

### `ResolveWaitAllAdmissionAcceptedAttached`
- From: `Attached`
- On: `ResolveWaitAllAdmission`(wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token, duplicate_operation_id, not_found_operation_id)
- Guards:
  - `no_duplicate_observed`
  - `wait_inactive`
  - `operations_tracked`
  - `operation_token_witness_valid`
- Emits: `WaitAllAdmissionResolved`
- To: `Attached`

### `ResolveWaitAllAdmissionAcceptedRunning`
- From: `Running`
- On: `ResolveWaitAllAdmission`(wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token, duplicate_operation_id, not_found_operation_id)
- Guards:
  - `no_duplicate_observed`
  - `wait_inactive`
  - `operations_tracked`
  - `operation_token_witness_valid`
- Emits: `WaitAllAdmissionResolved`
- To: `Running`

### `ResolveWaitAllAdmissionAcceptedRetired`
- From: `Retired`
- On: `ResolveWaitAllAdmission`(wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token, duplicate_operation_id, not_found_operation_id)
- Guards:
  - `no_duplicate_observed`
  - `wait_inactive`
  - `operations_tracked`
  - `operation_token_witness_valid`
- Emits: `WaitAllAdmissionResolved`
- To: `Retired`

### `ResolveWaitAllAdmissionAcceptedStopped`
- From: `Stopped`
- On: `ResolveWaitAllAdmission`(wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token, duplicate_operation_id, not_found_operation_id)
- Guards:
  - `no_duplicate_observed`
  - `wait_inactive`
  - `operations_tracked`
  - `operation_token_witness_valid`
- Emits: `WaitAllAdmissionResolved`
- To: `Stopped`

### `RequestWaitAllIdle`
- From: `Idle`
- On: `RequestWaitAll`(run_id, wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token)
- Guards:
  - `no_duplicate_observed`
  - `wait_inactive`
  - `operations_tracked`
  - `operation_token_witness_valid`
- To: `Idle`

### `RequestWaitAllAttached`
- From: `Attached`
- On: `RequestWaitAll`(run_id, wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token)
- Guards:
  - `no_duplicate_observed`
  - `wait_inactive`
  - `operations_tracked`
  - `operation_token_witness_valid`
- To: `Attached`

### `RequestWaitAllRunning`
- From: `Running`
- On: `RequestWaitAll`(run_id, wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token)
- Guards:
  - `no_duplicate_observed`
  - `wait_inactive`
  - `operations_tracked`
  - `operation_token_witness_valid`
- To: `Running`

### `RequestWaitAllRetired`
- From: `Retired`
- On: `RequestWaitAll`(run_id, wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token)
- Guards:
  - `no_duplicate_observed`
  - `wait_inactive`
  - `operations_tracked`
  - `operation_token_witness_valid`
- To: `Retired`

### `RequestWaitAllStopped`
- From: `Stopped`
- On: `RequestWaitAll`(run_id, wait_request_id, operation_id_sequence, operation_ids, operation_id_tokens, operation_token_by_id, operation_id_by_token)
- Guards:
  - `no_duplicate_observed`
  - `wait_inactive`
  - `operations_tracked`
  - `operation_token_witness_valid`
- To: `Stopped`

### `SatisfyWaitAllIdle`
- From: `Idle`
- On: `SatisfyWaitAll`(wait_request_id, run_id, operation_id_tokens)
- Guards:
  - `wait_is_active`
  - `wait_request_matches`
  - `wait_run_matches`
  - `operation_tokens_match`
  - `all_members_terminal`
- Emits: `WaitAllSatisfied`
- To: `Idle`

### `SatisfyWaitAllAttached`
- From: `Attached`
- On: `SatisfyWaitAll`(wait_request_id, run_id, operation_id_tokens)
- Guards:
  - `wait_is_active`
  - `wait_request_matches`
  - `wait_run_matches`
  - `operation_tokens_match`
  - `all_members_terminal`
- Emits: `WaitAllSatisfied`
- To: `Attached`

### `SatisfyWaitAllRunning`
- From: `Running`
- On: `SatisfyWaitAll`(wait_request_id, run_id, operation_id_tokens)
- Guards:
  - `wait_is_active`
  - `wait_request_matches`
  - `wait_run_matches`
  - `operation_tokens_match`
  - `all_members_terminal`
- Emits: `WaitAllSatisfied`
- To: `Running`

### `SatisfyWaitAllRetired`
- From: `Retired`
- On: `SatisfyWaitAll`(wait_request_id, run_id, operation_id_tokens)
- Guards:
  - `wait_is_active`
  - `wait_request_matches`
  - `wait_run_matches`
  - `operation_tokens_match`
  - `all_members_terminal`
- Emits: `WaitAllSatisfied`
- To: `Retired`

### `SatisfyWaitAllStopped`
- From: `Stopped`
- On: `SatisfyWaitAll`(wait_request_id, run_id, operation_id_tokens)
- Guards:
  - `wait_is_active`
  - `wait_request_matches`
  - `wait_run_matches`
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

### `StageVisibilityFilterIdle`
- From: `Idle`
- On: `StageVisibilityFilter`(filter, witnesses)
- Guards:
  - `filter_witnesses_match_machine_catalog`
  - `active_filter_has_machine_catalog_witnesses`
  - `staged_filter_has_machine_catalog_witnesses`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Idle`

### `StageVisibilityFilterAttached`
- From: `Attached`
- On: `StageVisibilityFilter`(filter, witnesses)
- Guards:
  - `filter_witnesses_match_machine_catalog`
  - `active_filter_has_machine_catalog_witnesses`
  - `staged_filter_has_machine_catalog_witnesses`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Attached`

### `StageVisibilityFilterRunning`
- From: `Running`
- On: `StageVisibilityFilter`(filter, witnesses)
- Guards:
  - `filter_witnesses_match_machine_catalog`
  - `active_filter_has_machine_catalog_witnesses`
  - `staged_filter_has_machine_catalog_witnesses`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Running`

### `StageVisibilityFilterRetired`
- From: `Retired`
- On: `StageVisibilityFilter`(filter, witnesses)
- Guards:
  - `filter_witnesses_match_machine_catalog`
  - `active_filter_has_machine_catalog_witnesses`
  - `staged_filter_has_machine_catalog_witnesses`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Retired`

### `StageVisibilityFilterStopped`
- From: `Stopped`
- On: `StageVisibilityFilter`(filter, witnesses)
- Guards:
  - `filter_witnesses_match_machine_catalog`
  - `active_filter_has_machine_catalog_witnesses`
  - `staged_filter_has_machine_catalog_witnesses`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Stopped`

### `ReplaceFilterToolAuthorityCatalogIdle`
- From: `Idle`
- On: `ReplaceFilterToolAuthorityCatalog`(catalog)
- Guards:
  - `session_registered`
  - `filter_authority_catalog_has_identity`
  - `capability_base_filter_matches_generated_llm_surface`
  - `inherited_filter_matches_replacement_catalog`
  - `active_filter_matches_replacement_catalog`
  - `staged_filter_matches_replacement_catalog`
  - `turn_overlay_allow_matches_replacement_catalog`
  - `turn_overlay_deny_matches_replacement_catalog`
- To: `Idle`

### `ReplaceFilterToolAuthorityCatalogAttached`
- From: `Attached`
- On: `ReplaceFilterToolAuthorityCatalog`(catalog)
- Guards:
  - `session_registered`
  - `filter_authority_catalog_has_identity`
  - `capability_base_filter_matches_generated_llm_surface`
  - `inherited_filter_matches_replacement_catalog`
  - `active_filter_matches_replacement_catalog`
  - `staged_filter_matches_replacement_catalog`
  - `turn_overlay_allow_matches_replacement_catalog`
  - `turn_overlay_deny_matches_replacement_catalog`
- To: `Attached`

### `ReplaceFilterToolAuthorityCatalogRunning`
- From: `Running`
- On: `ReplaceFilterToolAuthorityCatalog`(catalog)
- Guards:
  - `session_registered`
  - `filter_authority_catalog_has_identity`
  - `capability_base_filter_matches_generated_llm_surface`
  - `inherited_filter_matches_replacement_catalog`
  - `active_filter_matches_replacement_catalog`
  - `staged_filter_matches_replacement_catalog`
  - `turn_overlay_allow_matches_replacement_catalog`
  - `turn_overlay_deny_matches_replacement_catalog`
- To: `Running`

### `ReplaceFilterToolAuthorityCatalogRetired`
- From: `Retired`
- On: `ReplaceFilterToolAuthorityCatalog`(catalog)
- Guards:
  - `session_registered`
  - `filter_authority_catalog_has_identity`
  - `capability_base_filter_matches_generated_llm_surface`
  - `inherited_filter_matches_replacement_catalog`
  - `active_filter_matches_replacement_catalog`
  - `staged_filter_matches_replacement_catalog`
  - `turn_overlay_allow_matches_replacement_catalog`
  - `turn_overlay_deny_matches_replacement_catalog`
- To: `Retired`

### `ReplaceFilterToolAuthorityCatalogStopped`
- From: `Stopped`
- On: `ReplaceFilterToolAuthorityCatalog`(catalog)
- Guards:
  - `session_registered`
  - `filter_authority_catalog_has_identity`
  - `capability_base_filter_matches_generated_llm_surface`
  - `inherited_filter_matches_replacement_catalog`
  - `active_filter_matches_replacement_catalog`
  - `staged_filter_matches_replacement_catalog`
  - `turn_overlay_allow_matches_replacement_catalog`
  - `turn_overlay_deny_matches_replacement_catalog`
- To: `Stopped`

### `CommitVisibilityFilterIdle`
- From: `Idle`
- On: `CommitVisibilityFilter`(filter, revision)
- Guards:
  - `filter_matches_machine_staged_filter`
  - `revision_matches_machine_staged_revision`
  - `staged_filter_witnesses_match_machine_catalog`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Idle`

### `CommitVisibilityFilterAttached`
- From: `Attached`
- On: `CommitVisibilityFilter`(filter, revision)
- Guards:
  - `filter_matches_machine_staged_filter`
  - `revision_matches_machine_staged_revision`
  - `staged_filter_witnesses_match_machine_catalog`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Attached`

### `CommitVisibilityFilterRunning`
- From: `Running`
- On: `CommitVisibilityFilter`(filter, revision)
- Guards:
  - `filter_matches_machine_staged_filter`
  - `revision_matches_machine_staged_revision`
  - `staged_filter_witnesses_match_machine_catalog`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Running`

### `CommitVisibilityFilterRetired`
- From: `Retired`
- On: `CommitVisibilityFilter`(filter, revision)
- Guards:
  - `filter_matches_machine_staged_filter`
  - `revision_matches_machine_staged_revision`
  - `staged_filter_witnesses_match_machine_catalog`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Retired`

### `CommitVisibilityFilterStopped`
- From: `Stopped`
- On: `CommitVisibilityFilter`(filter, revision)
- Guards:
  - `filter_matches_machine_staged_filter`
  - `revision_matches_machine_staged_revision`
  - `staged_filter_witnesses_match_machine_catalog`
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

### `ReplaceDeferredToolAuthorityCatalogIdle`
- From: `Idle`
- On: `ReplaceDeferredToolAuthorityCatalog`(catalog)
- Guards:
  - `session_registered`
  - `deferred_authority_catalog_has_identity`
  - `active_deferred_authorities_match_replacement_catalog`
  - `staged_deferred_authorities_match_replacement_catalog`
  - `requested_witnesses_match_live_replacement_catalog`
- To: `Idle`

### `ReplaceDeferredToolAuthorityCatalogAttached`
- From: `Attached`
- On: `ReplaceDeferredToolAuthorityCatalog`(catalog)
- Guards:
  - `session_registered`
  - `deferred_authority_catalog_has_identity`
  - `active_deferred_authorities_match_replacement_catalog`
  - `staged_deferred_authorities_match_replacement_catalog`
  - `requested_witnesses_match_live_replacement_catalog`
- To: `Attached`

### `ReplaceDeferredToolAuthorityCatalogRunning`
- From: `Running`
- On: `ReplaceDeferredToolAuthorityCatalog`(catalog)
- Guards:
  - `session_registered`
  - `deferred_authority_catalog_has_identity`
  - `active_deferred_authorities_match_replacement_catalog`
  - `staged_deferred_authorities_match_replacement_catalog`
  - `requested_witnesses_match_live_replacement_catalog`
- To: `Running`

### `ReplaceDeferredToolAuthorityCatalogRetired`
- From: `Retired`
- On: `ReplaceDeferredToolAuthorityCatalog`(catalog)
- Guards:
  - `session_registered`
  - `deferred_authority_catalog_has_identity`
  - `active_deferred_authorities_match_replacement_catalog`
  - `staged_deferred_authorities_match_replacement_catalog`
  - `requested_witnesses_match_live_replacement_catalog`
- To: `Retired`

### `ReplaceDeferredToolAuthorityCatalogStopped`
- From: `Stopped`
- On: `ReplaceDeferredToolAuthorityCatalog`(catalog)
- Guards:
  - `session_registered`
  - `deferred_authority_catalog_has_identity`
  - `active_deferred_authorities_match_replacement_catalog`
  - `staged_deferred_authorities_match_replacement_catalog`
  - `requested_witnesses_match_live_replacement_catalog`
- To: `Stopped`

### `CommitDeferredNamesIdle`
- From: `Idle`
- On: `CommitDeferredNames`(authorities)
- Guards:
  - `authorities_match_machine_staged_authorities`
  - `authority_names_match_machine_staged_names`
  - `deferred_authorities_have_identity`
  - `deferred_authorities_match_machine_catalog`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Idle`

### `CommitDeferredNamesAttached`
- From: `Attached`
- On: `CommitDeferredNames`(authorities)
- Guards:
  - `authorities_match_machine_staged_authorities`
  - `authority_names_match_machine_staged_names`
  - `deferred_authorities_have_identity`
  - `deferred_authorities_match_machine_catalog`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Attached`

### `CommitDeferredNamesRunning`
- From: `Running`
- On: `CommitDeferredNames`(authorities)
- Guards:
  - `authorities_match_machine_staged_authorities`
  - `authority_names_match_machine_staged_names`
  - `deferred_authorities_have_identity`
  - `deferred_authorities_match_machine_catalog`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Running`

### `CommitDeferredNamesRetired`
- From: `Retired`
- On: `CommitDeferredNames`(authorities)
- Guards:
  - `authorities_match_machine_staged_authorities`
  - `authority_names_match_machine_staged_names`
  - `deferred_authorities_have_identity`
  - `deferred_authorities_match_machine_catalog`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Retired`

### `CommitDeferredNamesStopped`
- From: `Stopped`
- On: `CommitDeferredNames`(authorities)
- Guards:
  - `authorities_match_machine_staged_authorities`
  - `authority_names_match_machine_staged_names`
  - `deferred_authorities_have_identity`
  - `deferred_authorities_match_machine_catalog`
- Emits: `RefreshVisibleSurfaceSet`
- To: `Stopped`

### `SetTurnToolOverlayIdle`
- From: `Idle`
- On: `SetTurnToolOverlay`(allow_active, allow_names, deny_names)
- Guards:
  - `session_registered`
  - `inactive_allow_carries_no_names`
  - `allow_names_match_filter_authority_catalog`
  - `deny_names_match_filter_authority_catalog`
- To: `Idle`

### `SetTurnToolOverlayAttached`
- From: `Attached`
- On: `SetTurnToolOverlay`(allow_active, allow_names, deny_names)
- Guards:
  - `session_registered`
  - `inactive_allow_carries_no_names`
  - `allow_names_match_filter_authority_catalog`
  - `deny_names_match_filter_authority_catalog`
- To: `Attached`

### `SetTurnToolOverlayRunning`
- From: `Running`
- On: `SetTurnToolOverlay`(allow_active, allow_names, deny_names)
- Guards:
  - `session_registered`
  - `inactive_allow_carries_no_names`
  - `allow_names_match_filter_authority_catalog`
  - `deny_names_match_filter_authority_catalog`
- To: `Running`

### `SetTurnToolOverlayRetired`
- From: `Retired`
- On: `SetTurnToolOverlay`(allow_active, allow_names, deny_names)
- Guards:
  - `session_registered`
  - `inactive_allow_carries_no_names`
  - `allow_names_match_filter_authority_catalog`
  - `deny_names_match_filter_authority_catalog`
- To: `Retired`

### `SetTurnToolOverlayStopped`
- From: `Stopped`
- On: `SetTurnToolOverlay`(allow_active, allow_names, deny_names)
- Guards:
  - `session_registered`
  - `inactive_allow_carries_no_names`
  - `allow_names_match_filter_authority_catalog`
  - `deny_names_match_filter_authority_catalog`
- To: `Stopped`

### `ClearTurnToolOverlayIdle`
- From: `Idle`
- On: `ClearTurnToolOverlay`()
- Guards:
  - `session_registered`
- To: `Idle`

### `ClearTurnToolOverlayAttached`
- From: `Attached`
- On: `ClearTurnToolOverlay`()
- Guards:
  - `session_registered`
- To: `Attached`

### `ClearTurnToolOverlayRunning`
- From: `Running`
- On: `ClearTurnToolOverlay`()
- Guards:
  - `session_registered`
- To: `Running`

### `ClearTurnToolOverlayRetired`
- From: `Retired`
- On: `ClearTurnToolOverlay`()
- Guards:
  - `session_registered`
- To: `Retired`

### `ClearTurnToolOverlayStopped`
- From: `Stopped`
- On: `ClearTurnToolOverlay`()
- Guards:
  - `session_registered`
- To: `Stopped`

### `SyncVisibilityRevisionsIdle`
- From: `Idle`
- On: `SyncVisibilityRevisions`(capability_base_filter, inherited_base_filter, active_filter, staged_filter, active_revision, staged_revision, active_deferred_names, staged_deferred_names, requested_witnesses, filter_witnesses, active_deferred_authorities, staged_deferred_authorities)
- Guards:
  - `session_registered`
  - `visibility_state_replacement_matches_fields`
  - `capability_base_filter_matches_current_surface`
  - `equal_revision_requires_equal_active_and_staged_input`
  - `active_deferred_authorities_cover_names`
  - `staged_deferred_authorities_cover_names`
  - `active_deferred_authorities_are_name_scoped`
  - `staged_deferred_authorities_are_name_scoped`
  - `active_deferred_authorities_have_identity`
  - `staged_deferred_authorities_have_identity`
- To: `Idle`

### `SyncVisibilityRevisionsAttached`
- From: `Attached`
- On: `SyncVisibilityRevisions`(capability_base_filter, inherited_base_filter, active_filter, staged_filter, active_revision, staged_revision, active_deferred_names, staged_deferred_names, requested_witnesses, filter_witnesses, active_deferred_authorities, staged_deferred_authorities)
- Guards:
  - `session_registered`
  - `visibility_state_replacement_matches_fields`
  - `capability_base_filter_matches_current_surface`
  - `equal_revision_requires_equal_active_and_staged_input`
  - `active_deferred_authorities_cover_names`
  - `staged_deferred_authorities_cover_names`
  - `active_deferred_authorities_are_name_scoped`
  - `staged_deferred_authorities_are_name_scoped`
  - `active_deferred_authorities_have_identity`
  - `staged_deferred_authorities_have_identity`
- To: `Attached`

### `SyncVisibilityRevisionsRunning`
- From: `Running`
- On: `SyncVisibilityRevisions`(capability_base_filter, inherited_base_filter, active_filter, staged_filter, active_revision, staged_revision, active_deferred_names, staged_deferred_names, requested_witnesses, filter_witnesses, active_deferred_authorities, staged_deferred_authorities)
- Guards:
  - `session_registered`
  - `visibility_state_replacement_matches_fields`
  - `capability_base_filter_matches_current_surface`
  - `equal_revision_requires_equal_active_and_staged_input`
  - `active_deferred_authorities_cover_names`
  - `staged_deferred_authorities_cover_names`
  - `active_deferred_authorities_are_name_scoped`
  - `staged_deferred_authorities_are_name_scoped`
  - `active_deferred_authorities_have_identity`
  - `staged_deferred_authorities_have_identity`
- To: `Running`

### `SyncVisibilityRevisionsRetired`
- From: `Retired`
- On: `SyncVisibilityRevisions`(capability_base_filter, inherited_base_filter, active_filter, staged_filter, active_revision, staged_revision, active_deferred_names, staged_deferred_names, requested_witnesses, filter_witnesses, active_deferred_authorities, staged_deferred_authorities)
- Guards:
  - `session_registered`
  - `visibility_state_replacement_matches_fields`
  - `capability_base_filter_matches_current_surface`
  - `equal_revision_requires_equal_active_and_staged_input`
  - `active_deferred_authorities_cover_names`
  - `staged_deferred_authorities_cover_names`
  - `active_deferred_authorities_are_name_scoped`
  - `staged_deferred_authorities_are_name_scoped`
  - `active_deferred_authorities_have_identity`
  - `staged_deferred_authorities_have_identity`
- To: `Retired`

### `SyncVisibilityRevisionsStopped`
- From: `Stopped`
- On: `SyncVisibilityRevisions`(capability_base_filter, inherited_base_filter, active_filter, staged_filter, active_revision, staged_revision, active_deferred_names, staged_deferred_names, requested_witnesses, filter_witnesses, active_deferred_authorities, staged_deferred_authorities)
- Guards:
  - `session_registered`
  - `visibility_state_replacement_matches_fields`
  - `capability_base_filter_matches_current_surface`
  - `equal_revision_requires_equal_active_and_staged_input`
  - `active_deferred_authorities_cover_names`
  - `staged_deferred_authorities_cover_names`
  - `active_deferred_authorities_are_name_scoped`
  - `staged_deferred_authorities_are_name_scoped`
  - `active_deferred_authorities_have_identity`
  - `staged_deferred_authorities_have_identity`
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
- On: `PeerRequestSent`(corr_id)
- Guards:
  - `not_already_pending`
- Emits: `PeerInteractionStateChanged`
- To: `Idle`

### `PeerRequestSentAttached`
- From: `Attached`
- On: `PeerRequestSent`(corr_id)
- Guards:
  - `not_already_pending`
- Emits: `PeerInteractionStateChanged`
- To: `Attached`

### `PeerRequestSentRunning`
- From: `Running`
- On: `PeerRequestSent`(corr_id)
- Guards:
  - `not_already_pending`
- Emits: `PeerInteractionStateChanged`
- To: `Running`

### `PeerRequestSentRetired`
- From: `Retired`
- On: `PeerRequestSent`(corr_id)
- Guards:
  - `not_already_pending`
- Emits: `PeerInteractionStateChanged`
- To: `Retired`

### `PeerRequestSentStopped`
- From: `Stopped`
- On: `PeerRequestSent`(corr_id)
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

### `PeerResponseRejectedIdle`
- From: `Idle`
- On: `PeerResponseRejected`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Idle`

### `PeerResponseRejectedAttached`
- From: `Attached`
- On: `PeerResponseRejected`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Attached`

### `PeerResponseRejectedRunning`
- From: `Running`
- On: `PeerResponseRejected`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Running`

### `PeerResponseRejectedRetired`
- From: `Retired`
- On: `PeerResponseRejected`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Retired`

### `PeerResponseRejectedStopped`
- From: `Stopped`
- On: `PeerResponseRejected`(corr_id)
- Guards:
  - `pending_exists`
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

### `PeerRequestSendFailedIdle`
- From: `Idle`
- On: `PeerRequestSendFailed`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Idle`

### `PeerRequestSendFailedAttached`
- From: `Attached`
- On: `PeerRequestSendFailed`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Attached`

### `PeerRequestSendFailedRunning`
- From: `Running`
- On: `PeerRequestSendFailed`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Running`

### `PeerRequestSendFailedRetired`
- From: `Retired`
- On: `PeerRequestSendFailed`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Retired`

### `PeerRequestSendFailedStopped`
- From: `Stopped`
- On: `PeerRequestSendFailed`(corr_id)
- Guards:
  - `pending_exists`
- Emits: `PeerInteractionStateChanged`, `PeerInteractionCleanup`
- To: `Stopped`

### `PeerRequestReceivedIdle`
- From: `Idle`
- On: `PeerRequestReceived`(corr_id, handling_mode)
- Guards:
  - `not_already_inbound`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Idle`

### `PeerRequestReceivedAttached`
- From: `Attached`
- On: `PeerRequestReceived`(corr_id, handling_mode)
- Guards:
  - `not_already_inbound`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Attached`

### `PeerRequestReceivedRunning`
- From: `Running`
- On: `PeerRequestReceived`(corr_id, handling_mode)
- Guards:
  - `not_already_inbound`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Running`

### `PeerRequestReceivedRetired`
- From: `Retired`
- On: `PeerRequestReceived`(corr_id, handling_mode)
- Guards:
  - `not_already_inbound`
- Emits: `InboundPeerInteractionStateChanged`
- To: `Retired`

### `PeerRequestReceivedStopped`
- From: `Stopped`
- On: `PeerRequestReceived`(corr_id, handling_mode)
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

### `ResolveSupervisorBindAdmissionBootstrapIdle`
- From: `Idle`
- On: `ResolveSupervisorBindAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_unbound`
- Emits: `SupervisorBindAdmissionResolved`
- To: `Idle`

### `ResolveSupervisorBindAdmissionBootstrapAttached`
- From: `Attached`
- On: `ResolveSupervisorBindAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_unbound`
- Emits: `SupervisorBindAdmissionResolved`
- To: `Attached`

### `ResolveSupervisorBindAdmissionBootstrapRunning`
- From: `Running`
- On: `ResolveSupervisorBindAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_unbound`
- Emits: `SupervisorBindAdmissionResolved`
- To: `Running`

### `ResolveSupervisorBindAdmissionIdempotentAckIdle`
- From: `Idle`
- On: `ResolveSupervisorBindAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `sender_peer_id_matches_current`
- Emits: `SupervisorBindAdmissionResolved`
- To: `Idle`

### `ResolveSupervisorBindAdmissionIdempotentAckAttached`
- From: `Attached`
- On: `ResolveSupervisorBindAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `sender_peer_id_matches_current`
- Emits: `SupervisorBindAdmissionResolved`
- To: `Attached`

### `ResolveSupervisorBindAdmissionIdempotentAckRunning`
- From: `Running`
- On: `ResolveSupervisorBindAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `sender_peer_id_matches_current`
- Emits: `SupervisorBindAdmissionResolved`
- To: `Running`

### `ResolveSupervisorBindAdmissionSenderMismatchIdle`
- From: `Idle`
- On: `ResolveSupervisorBindAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `sender_peer_id_mismatch`
- Emits: `SupervisorBindAdmissionResolved`
- To: `Idle`

### `ResolveSupervisorBindAdmissionSenderMismatchAttached`
- From: `Attached`
- On: `ResolveSupervisorBindAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `sender_peer_id_mismatch`
- Emits: `SupervisorBindAdmissionResolved`
- To: `Attached`

### `ResolveSupervisorBindAdmissionSenderMismatchRunning`
- From: `Running`
- On: `ResolveSupervisorBindAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `sender_peer_id_mismatch`
- Emits: `SupervisorBindAdmissionResolved`
- To: `Running`

### `ResolveSupervisorBindAdmissionAlreadyBoundIdle`
- From: `Idle`
- On: `ResolveSupervisorBindAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_binding_mismatch`
- Emits: `SupervisorBindAdmissionResolved`
- To: `Idle`

### `ResolveSupervisorBindAdmissionAlreadyBoundAttached`
- From: `Attached`
- On: `ResolveSupervisorBindAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_binding_mismatch`
- Emits: `SupervisorBindAdmissionResolved`
- To: `Attached`

### `ResolveSupervisorBindAdmissionAlreadyBoundRunning`
- From: `Running`
- On: `ResolveSupervisorBindAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_binding_mismatch`
- Emits: `SupervisorBindAdmissionResolved`
- To: `Running`

### `ResolveSupervisorBindMaterialAdmissionAddressMismatchIdle`
- From: `Idle`
- On: `ResolveSupervisorBindMaterialAdmission`(address_matches, sender_matches_supervisor, expected_peer_id_matches, bootstrap_token_matches)
- Guards:
  - `address_mismatch`
- Emits: `SupervisorBindMaterialAdmissionResolved`
- To: `Idle`

### `ResolveSupervisorBindMaterialAdmissionAddressMismatchAttached`
- From: `Attached`
- On: `ResolveSupervisorBindMaterialAdmission`(address_matches, sender_matches_supervisor, expected_peer_id_matches, bootstrap_token_matches)
- Guards:
  - `address_mismatch`
- Emits: `SupervisorBindMaterialAdmissionResolved`
- To: `Attached`

### `ResolveSupervisorBindMaterialAdmissionAddressMismatchRunning`
- From: `Running`
- On: `ResolveSupervisorBindMaterialAdmission`(address_matches, sender_matches_supervisor, expected_peer_id_matches, bootstrap_token_matches)
- Guards:
  - `address_mismatch`
- Emits: `SupervisorBindMaterialAdmissionResolved`
- To: `Running`

### `ResolveSupervisorBindMaterialAdmissionSenderMismatchIdle`
- From: `Idle`
- On: `ResolveSupervisorBindMaterialAdmission`(address_matches, sender_matches_supervisor, expected_peer_id_matches, bootstrap_token_matches)
- Guards:
  - `address_matches`
  - `sender_mismatch`
- Emits: `SupervisorBindMaterialAdmissionResolved`
- To: `Idle`

### `ResolveSupervisorBindMaterialAdmissionSenderMismatchAttached`
- From: `Attached`
- On: `ResolveSupervisorBindMaterialAdmission`(address_matches, sender_matches_supervisor, expected_peer_id_matches, bootstrap_token_matches)
- Guards:
  - `address_matches`
  - `sender_mismatch`
- Emits: `SupervisorBindMaterialAdmissionResolved`
- To: `Attached`

### `ResolveSupervisorBindMaterialAdmissionSenderMismatchRunning`
- From: `Running`
- On: `ResolveSupervisorBindMaterialAdmission`(address_matches, sender_matches_supervisor, expected_peer_id_matches, bootstrap_token_matches)
- Guards:
  - `address_matches`
  - `sender_mismatch`
- Emits: `SupervisorBindMaterialAdmissionResolved`
- To: `Running`

### `ResolveSupervisorBindMaterialAdmissionInvalidPeerSpecIdle`
- From: `Idle`
- On: `ResolveSupervisorBindMaterialAdmission`(address_matches, sender_matches_supervisor, expected_peer_id_matches, bootstrap_token_matches)
- Guards:
  - `address_matches`
  - `sender_matches`
  - `expected_peer_id_mismatch`
- Emits: `SupervisorBindMaterialAdmissionResolved`
- To: `Idle`

### `ResolveSupervisorBindMaterialAdmissionInvalidPeerSpecAttached`
- From: `Attached`
- On: `ResolveSupervisorBindMaterialAdmission`(address_matches, sender_matches_supervisor, expected_peer_id_matches, bootstrap_token_matches)
- Guards:
  - `address_matches`
  - `sender_matches`
  - `expected_peer_id_mismatch`
- Emits: `SupervisorBindMaterialAdmissionResolved`
- To: `Attached`

### `ResolveSupervisorBindMaterialAdmissionInvalidPeerSpecRunning`
- From: `Running`
- On: `ResolveSupervisorBindMaterialAdmission`(address_matches, sender_matches_supervisor, expected_peer_id_matches, bootstrap_token_matches)
- Guards:
  - `address_matches`
  - `sender_matches`
  - `expected_peer_id_mismatch`
- Emits: `SupervisorBindMaterialAdmissionResolved`
- To: `Running`

### `ResolveSupervisorBindMaterialAdmissionInvalidBootstrapTokenIdle`
- From: `Idle`
- On: `ResolveSupervisorBindMaterialAdmission`(address_matches, sender_matches_supervisor, expected_peer_id_matches, bootstrap_token_matches)
- Guards:
  - `address_matches`
  - `sender_matches`
  - `expected_peer_id_matches`
  - `bootstrap_token_mismatch`
- Emits: `SupervisorBindMaterialAdmissionResolved`
- To: `Idle`

### `ResolveSupervisorBindMaterialAdmissionInvalidBootstrapTokenAttached`
- From: `Attached`
- On: `ResolveSupervisorBindMaterialAdmission`(address_matches, sender_matches_supervisor, expected_peer_id_matches, bootstrap_token_matches)
- Guards:
  - `address_matches`
  - `sender_matches`
  - `expected_peer_id_matches`
  - `bootstrap_token_mismatch`
- Emits: `SupervisorBindMaterialAdmissionResolved`
- To: `Attached`

### `ResolveSupervisorBindMaterialAdmissionInvalidBootstrapTokenRunning`
- From: `Running`
- On: `ResolveSupervisorBindMaterialAdmission`(address_matches, sender_matches_supervisor, expected_peer_id_matches, bootstrap_token_matches)
- Guards:
  - `address_matches`
  - `sender_matches`
  - `expected_peer_id_matches`
  - `bootstrap_token_mismatch`
- Emits: `SupervisorBindMaterialAdmissionResolved`
- To: `Running`

### `ResolveSupervisorBindMaterialAdmissionAcceptIdle`
- From: `Idle`
- On: `ResolveSupervisorBindMaterialAdmission`(address_matches, sender_matches_supervisor, expected_peer_id_matches, bootstrap_token_matches)
- Guards:
  - `address_matches`
  - `sender_matches`
  - `expected_peer_id_matches`
  - `bootstrap_token_matches`
- Emits: `SupervisorBindMaterialAdmissionResolved`
- To: `Idle`

### `ResolveSupervisorBindMaterialAdmissionAcceptAttached`
- From: `Attached`
- On: `ResolveSupervisorBindMaterialAdmission`(address_matches, sender_matches_supervisor, expected_peer_id_matches, bootstrap_token_matches)
- Guards:
  - `address_matches`
  - `sender_matches`
  - `expected_peer_id_matches`
  - `bootstrap_token_matches`
- Emits: `SupervisorBindMaterialAdmissionResolved`
- To: `Attached`

### `ResolveSupervisorBindMaterialAdmissionAcceptRunning`
- From: `Running`
- On: `ResolveSupervisorBindMaterialAdmission`(address_matches, sender_matches_supervisor, expected_peer_id_matches, bootstrap_token_matches)
- Guards:
  - `address_matches`
  - `sender_matches`
  - `expected_peer_id_matches`
  - `bootstrap_token_matches`
- Emits: `SupervisorBindMaterialAdmissionResolved`
- To: `Running`

### `ResolveTranscriptEditAdmissionIdleBusy`
- From: `Idle`
- On: `ResolveTranscriptEditAdmission`(runtime_running, has_active_inputs)
- Guards:
  - `busy_disjunction`
- Emits: `TranscriptEditAdmissionResolved`
- To: `Idle`

### `ResolveTranscriptEditAdmissionIdleAdmissible`
- From: `Idle`
- On: `ResolveTranscriptEditAdmission`(runtime_running, has_active_inputs)
- Guards:
  - `idle_no_active`
- Emits: `TranscriptEditAdmissionResolved`
- To: `Idle`

### `ResolveTranscriptEditAdmissionAttachedBusy`
- From: `Attached`
- On: `ResolveTranscriptEditAdmission`(runtime_running, has_active_inputs)
- Guards:
  - `busy_disjunction`
- Emits: `TranscriptEditAdmissionResolved`
- To: `Attached`

### `ResolveTranscriptEditAdmissionAttachedAdmissible`
- From: `Attached`
- On: `ResolveTranscriptEditAdmission`(runtime_running, has_active_inputs)
- Guards:
  - `attached_no_active`
- Emits: `TranscriptEditAdmissionResolved`
- To: `Attached`

### `ResolveTranscriptEditAdmissionRunningBusy`
- From: `Running`
- On: `ResolveTranscriptEditAdmission`(runtime_running, has_active_inputs)
- Guards:
  - `busy_disjunction`
- Emits: `TranscriptEditAdmissionResolved`
- To: `Running`

### `ResolveTranscriptEditAdmissionRunningAdmissible`
- From: `Running`
- On: `ResolveTranscriptEditAdmission`(runtime_running, has_active_inputs)
- Guards:
  - `running_no_active`
- Emits: `TranscriptEditAdmissionResolved`
- To: `Running`

### `ResolveSupervisorAuthorizeAdmissionNotBoundIdle`
- From: `Idle`
- On: `ResolveSupervisorAuthorizeAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_not_bound`
- Emits: `SupervisorAuthorizeAdmissionResolved`
- To: `Idle`

### `ResolveSupervisorAuthorizeAdmissionNotBoundAttached`
- From: `Attached`
- On: `ResolveSupervisorAuthorizeAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_not_bound`
- Emits: `SupervisorAuthorizeAdmissionResolved`
- To: `Attached`

### `ResolveSupervisorAuthorizeAdmissionNotBoundRunning`
- From: `Running`
- On: `ResolveSupervisorAuthorizeAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_not_bound`
- Emits: `SupervisorAuthorizeAdmissionResolved`
- To: `Running`

### `ResolveSupervisorAuthorizeAdmissionStaleSupervisorIdle`
- From: `Idle`
- On: `ResolveSupervisorAuthorizeAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `stale_supervisor_epoch`
- Emits: `SupervisorAuthorizeAdmissionResolved`
- To: `Idle`

### `ResolveSupervisorAuthorizeAdmissionStaleSupervisorAttached`
- From: `Attached`
- On: `ResolveSupervisorAuthorizeAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `stale_supervisor_epoch`
- Emits: `SupervisorAuthorizeAdmissionResolved`
- To: `Attached`

### `ResolveSupervisorAuthorizeAdmissionStaleSupervisorRunning`
- From: `Running`
- On: `ResolveSupervisorAuthorizeAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `stale_supervisor_epoch`
- Emits: `SupervisorAuthorizeAdmissionResolved`
- To: `Running`

### `ResolveSupervisorAuthorizeAdmissionSenderMismatchIdle`
- From: `Idle`
- On: `ResolveSupervisorAuthorizeAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_epoch_not_stale`
  - `sender_peer_id_mismatch`
- Emits: `SupervisorAuthorizeAdmissionResolved`
- To: `Idle`

### `ResolveSupervisorAuthorizeAdmissionSenderMismatchAttached`
- From: `Attached`
- On: `ResolveSupervisorAuthorizeAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_epoch_not_stale`
  - `sender_peer_id_mismatch`
- Emits: `SupervisorAuthorizeAdmissionResolved`
- To: `Attached`

### `ResolveSupervisorAuthorizeAdmissionSenderMismatchRunning`
- From: `Running`
- On: `ResolveSupervisorAuthorizeAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_epoch_not_stale`
  - `sender_peer_id_mismatch`
- Emits: `SupervisorAuthorizeAdmissionResolved`
- To: `Running`

### `ResolveSupervisorAuthorizeAdmissionIdempotentAckIdle`
- From: `Idle`
- On: `ResolveSupervisorAuthorizeAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `sender_peer_id_matches_current`
- Emits: `SupervisorAuthorizeAdmissionResolved`
- To: `Idle`

### `ResolveSupervisorAuthorizeAdmissionIdempotentAckAttached`
- From: `Attached`
- On: `ResolveSupervisorAuthorizeAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `sender_peer_id_matches_current`
- Emits: `SupervisorAuthorizeAdmissionResolved`
- To: `Attached`

### `ResolveSupervisorAuthorizeAdmissionIdempotentAckRunning`
- From: `Running`
- On: `ResolveSupervisorAuthorizeAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `sender_peer_id_matches_current`
- Emits: `SupervisorAuthorizeAdmissionResolved`
- To: `Running`

### `ResolveSupervisorAuthorizeAdmissionProceedIdle`
- From: `Idle`
- On: `ResolveSupervisorAuthorizeAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_epoch_not_stale`
  - `sender_peer_id_matches_current`
  - `not_idempotent`
- Emits: `SupervisorAuthorizeAdmissionResolved`
- To: `Idle`

### `ResolveSupervisorAuthorizeAdmissionProceedAttached`
- From: `Attached`
- On: `ResolveSupervisorAuthorizeAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_epoch_not_stale`
  - `sender_peer_id_matches_current`
  - `not_idempotent`
- Emits: `SupervisorAuthorizeAdmissionResolved`
- To: `Attached`

### `ResolveSupervisorAuthorizeAdmissionProceedRunning`
- From: `Running`
- On: `ResolveSupervisorAuthorizeAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_epoch_not_stale`
  - `sender_peer_id_matches_current`
  - `not_idempotent`
- Emits: `SupervisorAuthorizeAdmissionResolved`
- To: `Running`

### `BindSupervisorIdle`
- From: `Idle`
- On: `BindSupervisor`(name, peer_id, address, signing_public_key, epoch)
- Guards:
  - `supervisor_unbound`
- Emits: `PublishSupervisorTrustEdge`
- To: `Idle`

### `BindSupervisorAttached`
- From: `Attached`
- On: `BindSupervisor`(name, peer_id, address, signing_public_key, epoch)
- Guards:
  - `supervisor_unbound`
- Emits: `PublishSupervisorTrustEdge`
- To: `Attached`

### `BindSupervisorRunning`
- From: `Running`
- On: `BindSupervisor`(name, peer_id, address, signing_public_key, epoch)
- Guards:
  - `supervisor_unbound`
- Emits: `PublishSupervisorTrustEdge`
- To: `Running`

### `BindSupervisorRetired`
- From: `Retired`
- On: `BindSupervisor`(name, peer_id, address, signing_public_key, epoch)
- Guards:
  - `supervisor_unbound`
- Emits: `PublishSupervisorTrustEdge`
- To: `Retired`

### `BindSupervisorStopped`
- From: `Stopped`
- On: `BindSupervisor`(name, peer_id, address, signing_public_key, epoch)
- Guards:
  - `supervisor_unbound`
- Emits: `PublishSupervisorTrustEdge`
- To: `Stopped`

### `AuthorizeSupervisorIdle`
- From: `Idle`
- On: `AuthorizeSupervisor`(name, peer_id, address, signing_public_key, epoch)
- Guards:
  - `supervisor_bound`
- Emits: `PublishSupervisorTrustEdge`
- To: `Idle`

### `AuthorizeSupervisorAttached`
- From: `Attached`
- On: `AuthorizeSupervisor`(name, peer_id, address, signing_public_key, epoch)
- Guards:
  - `supervisor_bound`
- Emits: `PublishSupervisorTrustEdge`
- To: `Attached`

### `AuthorizeSupervisorRunning`
- From: `Running`
- On: `AuthorizeSupervisor`(name, peer_id, address, signing_public_key, epoch)
- Guards:
  - `supervisor_bound`
- Emits: `PublishSupervisorTrustEdge`
- To: `Running`

### `AuthorizeSupervisorRetired`
- From: `Retired`
- On: `AuthorizeSupervisor`(name, peer_id, address, signing_public_key, epoch)
- Guards:
  - `supervisor_bound`
- Emits: `PublishSupervisorTrustEdge`
- To: `Retired`

### `AuthorizeSupervisorStopped`
- From: `Stopped`
- On: `AuthorizeSupervisor`(name, peer_id, address, signing_public_key, epoch)
- Guards:
  - `supervisor_bound`
- Emits: `PublishSupervisorTrustEdge`
- To: `Stopped`

### `RequestSupervisorTrustPublishIdle`
- From: `Idle`
- On: `RequestSupervisorTrustPublish`(name, peer_id, address, signing_public_key, epoch)
- Guards:
  - `supervisor_bound`
  - `name_matches_current`
  - `peer_id_matches_current`
  - `address_matches_current`
  - `signing_public_key_matches_current`
  - `epoch_matches_current`
- Emits: `PublishSupervisorTrustEdge`
- To: `Idle`

### `RequestSupervisorTrustPublishAttached`
- From: `Attached`
- On: `RequestSupervisorTrustPublish`(name, peer_id, address, signing_public_key, epoch)
- Guards:
  - `supervisor_bound`
  - `name_matches_current`
  - `peer_id_matches_current`
  - `address_matches_current`
  - `signing_public_key_matches_current`
  - `epoch_matches_current`
- Emits: `PublishSupervisorTrustEdge`
- To: `Attached`

### `RequestSupervisorTrustPublishRunning`
- From: `Running`
- On: `RequestSupervisorTrustPublish`(name, peer_id, address, signing_public_key, epoch)
- Guards:
  - `supervisor_bound`
  - `name_matches_current`
  - `peer_id_matches_current`
  - `address_matches_current`
  - `signing_public_key_matches_current`
  - `epoch_matches_current`
- Emits: `PublishSupervisorTrustEdge`
- To: `Running`

### `RequestSupervisorTrustPublishRetired`
- From: `Retired`
- On: `RequestSupervisorTrustPublish`(name, peer_id, address, signing_public_key, epoch)
- Guards:
  - `supervisor_bound`
  - `name_matches_current`
  - `peer_id_matches_current`
  - `address_matches_current`
  - `signing_public_key_matches_current`
  - `epoch_matches_current`
- Emits: `PublishSupervisorTrustEdge`
- To: `Retired`

### `RequestSupervisorTrustPublishStopped`
- From: `Stopped`
- On: `RequestSupervisorTrustPublish`(name, peer_id, address, signing_public_key, epoch)
- Guards:
  - `supervisor_bound`
  - `name_matches_current`
  - `peer_id_matches_current`
  - `address_matches_current`
  - `signing_public_key_matches_current`
  - `epoch_matches_current`
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
  - `peer_id_matches_pending_publish`
  - `epoch_matches_pending_publish`
- To: `Idle`

### `SupervisorTrustEdgePublishedAttached`
- From: `Attached`
- On: `SupervisorTrustEdgePublished`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
  - `peer_id_matches_pending_publish`
  - `epoch_matches_pending_publish`
- To: `Attached`

### `SupervisorTrustEdgePublishedRunning`
- From: `Running`
- On: `SupervisorTrustEdgePublished`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
  - `peer_id_matches_pending_publish`
  - `epoch_matches_pending_publish`
- To: `Running`

### `SupervisorTrustEdgePublishedRetired`
- From: `Retired`
- On: `SupervisorTrustEdgePublished`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
  - `peer_id_matches_pending_publish`
  - `epoch_matches_pending_publish`
- To: `Retired`

### `SupervisorTrustEdgePublishedStopped`
- From: `Stopped`
- On: `SupervisorTrustEdgePublished`(peer_id, epoch)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
  - `peer_id_matches_pending_publish`
  - `epoch_matches_pending_publish`
- To: `Stopped`

### `SupervisorTrustEdgePublishFailedIdle`
- From: `Idle`
- On: `SupervisorTrustEdgePublishFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
  - `peer_id_matches_pending_publish`
  - `epoch_matches_pending_publish`
- To: `Idle`

### `SupervisorTrustEdgePublishFailedAttached`
- From: `Attached`
- On: `SupervisorTrustEdgePublishFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
  - `peer_id_matches_pending_publish`
  - `epoch_matches_pending_publish`
- To: `Attached`

### `SupervisorTrustEdgePublishFailedRunning`
- From: `Running`
- On: `SupervisorTrustEdgePublishFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
  - `peer_id_matches_pending_publish`
  - `epoch_matches_pending_publish`
- To: `Running`

### `SupervisorTrustEdgePublishFailedRetired`
- From: `Retired`
- On: `SupervisorTrustEdgePublishFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
  - `peer_id_matches_pending_publish`
  - `epoch_matches_pending_publish`
- To: `Retired`

### `SupervisorTrustEdgePublishFailedStopped`
- From: `Stopped`
- On: `SupervisorTrustEdgePublishFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_bound`
  - `peer_id_matches_current`
  - `epoch_matches_current`
  - `peer_id_matches_pending_publish`
  - `epoch_matches_pending_publish`
- To: `Stopped`

### `SupervisorTrustEdgeRevokedIdle`
- From: `Idle`
- On: `SupervisorTrustEdgeRevoked`(peer_id, epoch)
- Guards:
  - `supervisor_unbound`
  - `peer_id_matches_pending_revoke`
  - `epoch_matches_pending_revoke`
- To: `Idle`

### `SupervisorTrustEdgeRevokedAttached`
- From: `Attached`
- On: `SupervisorTrustEdgeRevoked`(peer_id, epoch)
- Guards:
  - `supervisor_unbound`
  - `peer_id_matches_pending_revoke`
  - `epoch_matches_pending_revoke`
- To: `Attached`

### `SupervisorTrustEdgeRevokedRunning`
- From: `Running`
- On: `SupervisorTrustEdgeRevoked`(peer_id, epoch)
- Guards:
  - `supervisor_unbound`
  - `peer_id_matches_pending_revoke`
  - `epoch_matches_pending_revoke`
- To: `Running`

### `SupervisorTrustEdgeRevokedRetired`
- From: `Retired`
- On: `SupervisorTrustEdgeRevoked`(peer_id, epoch)
- Guards:
  - `supervisor_unbound`
  - `peer_id_matches_pending_revoke`
  - `epoch_matches_pending_revoke`
- To: `Retired`

### `SupervisorTrustEdgeRevokedStopped`
- From: `Stopped`
- On: `SupervisorTrustEdgeRevoked`(peer_id, epoch)
- Guards:
  - `supervisor_unbound`
  - `peer_id_matches_pending_revoke`
  - `epoch_matches_pending_revoke`
- To: `Stopped`

### `SupervisorTrustEdgeRevokeFailedIdle`
- From: `Idle`
- On: `SupervisorTrustEdgeRevokeFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_unbound`
  - `peer_id_matches_pending_revoke`
  - `epoch_matches_pending_revoke`
- To: `Idle`

### `SupervisorTrustEdgeRevokeFailedAttached`
- From: `Attached`
- On: `SupervisorTrustEdgeRevokeFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_unbound`
  - `peer_id_matches_pending_revoke`
  - `epoch_matches_pending_revoke`
- To: `Attached`

### `SupervisorTrustEdgeRevokeFailedRunning`
- From: `Running`
- On: `SupervisorTrustEdgeRevokeFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_unbound`
  - `peer_id_matches_pending_revoke`
  - `epoch_matches_pending_revoke`
- To: `Running`

### `SupervisorTrustEdgeRevokeFailedRetired`
- From: `Retired`
- On: `SupervisorTrustEdgeRevokeFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_unbound`
  - `peer_id_matches_pending_revoke`
  - `epoch_matches_pending_revoke`
- To: `Retired`

### `SupervisorTrustEdgeRevokeFailedStopped`
- From: `Stopped`
- On: `SupervisorTrustEdgeRevokeFailed`(peer_id, epoch, reason)
- Guards:
  - `supervisor_unbound`
  - `peer_id_matches_pending_revoke`
  - `epoch_matches_pending_revoke`
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

### `RepairAddDirectPeerEndpointIdle`
- From: `Idle`
- On: `AddDirectPeerEndpoint`(endpoint)
- Guards:
  - `endpoint_already_direct`
- Emits: `CommsTrustReconcileRequested`
- To: `Idle`

### `RepairAddDirectPeerEndpointAttached`
- From: `Attached`
- On: `AddDirectPeerEndpoint`(endpoint)
- Guards:
  - `endpoint_already_direct`
- Emits: `CommsTrustReconcileRequested`
- To: `Attached`

### `RepairAddDirectPeerEndpointRunning`
- From: `Running`
- On: `AddDirectPeerEndpoint`(endpoint)
- Guards:
  - `endpoint_already_direct`
- Emits: `CommsTrustReconcileRequested`
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

### `RepairRemoveDirectPeerEndpointIdle`
- From: `Idle`
- On: `RemoveDirectPeerEndpoint`(endpoint)
- Guards:
  - `endpoint_absent_from_direct`
- Emits: `CommsTrustReconcileRequested`
- To: `Idle`

### `RepairRemoveDirectPeerEndpointAttached`
- From: `Attached`
- On: `RemoveDirectPeerEndpoint`(endpoint)
- Guards:
  - `endpoint_absent_from_direct`
- Emits: `CommsTrustReconcileRequested`
- To: `Attached`

### `RepairRemoveDirectPeerEndpointRunning`
- From: `Running`
- On: `RemoveDirectPeerEndpoint`(endpoint)
- Guards:
  - `endpoint_absent_from_direct`
- Emits: `CommsTrustReconcileRequested`
- To: `Running`

### `ResolveSupervisorBridgeCommandAdmissionAcceptedIdle`
- From: `Idle`
- On: `ResolveSupervisorBridgeCommandAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `sender_peer_id_matches_current`
- Emits: `SupervisorBridgeCommandAdmissionResolved`
- To: `Idle`

### `ResolveSupervisorBridgeCommandAdmissionAcceptedAttached`
- From: `Attached`
- On: `ResolveSupervisorBridgeCommandAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `sender_peer_id_matches_current`
- Emits: `SupervisorBridgeCommandAdmissionResolved`
- To: `Attached`

### `ResolveSupervisorBridgeCommandAdmissionAcceptedRunning`
- From: `Running`
- On: `ResolveSupervisorBridgeCommandAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `sender_peer_id_matches_current`
- Emits: `SupervisorBridgeCommandAdmissionResolved`
- To: `Running`

### `ResolveSupervisorBridgeCommandAdmissionNotBoundIdle`
- From: `Idle`
- On: `ResolveSupervisorBridgeCommandAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_not_bound`
- Emits: `SupervisorBridgeCommandAdmissionResolved`
- To: `Idle`

### `ResolveSupervisorBridgeCommandAdmissionNotBoundAttached`
- From: `Attached`
- On: `ResolveSupervisorBridgeCommandAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_not_bound`
- Emits: `SupervisorBridgeCommandAdmissionResolved`
- To: `Attached`

### `ResolveSupervisorBridgeCommandAdmissionNotBoundRunning`
- From: `Running`
- On: `ResolveSupervisorBridgeCommandAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_not_bound`
- Emits: `SupervisorBridgeCommandAdmissionResolved`
- To: `Running`

### `ResolveSupervisorBridgeCommandAdmissionStaleSupervisorIdle`
- From: `Idle`
- On: `ResolveSupervisorBridgeCommandAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_binding_mismatch`
- Emits: `SupervisorBridgeCommandAdmissionResolved`
- To: `Idle`

### `ResolveSupervisorBridgeCommandAdmissionStaleSupervisorAttached`
- From: `Attached`
- On: `ResolveSupervisorBridgeCommandAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_binding_mismatch`
- Emits: `SupervisorBridgeCommandAdmissionResolved`
- To: `Attached`

### `ResolveSupervisorBridgeCommandAdmissionStaleSupervisorRunning`
- From: `Running`
- On: `ResolveSupervisorBridgeCommandAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_binding_mismatch`
- Emits: `SupervisorBridgeCommandAdmissionResolved`
- To: `Running`

### `ResolveSupervisorBridgeCommandAdmissionSenderMismatchIdle`
- From: `Idle`
- On: `ResolveSupervisorBridgeCommandAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `sender_peer_id_mismatch`
- Emits: `SupervisorBridgeCommandAdmissionResolved`
- To: `Idle`

### `ResolveSupervisorBridgeCommandAdmissionSenderMismatchAttached`
- From: `Attached`
- On: `ResolveSupervisorBridgeCommandAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `sender_peer_id_mismatch`
- Emits: `SupervisorBridgeCommandAdmissionResolved`
- To: `Attached`

### `ResolveSupervisorBridgeCommandAdmissionSenderMismatchRunning`
- From: `Running`
- On: `ResolveSupervisorBridgeCommandAdmission`(supervisor_peer_id, supervisor_epoch, sender_peer_id)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `sender_peer_id_mismatch`
- Emits: `SupervisorBridgeCommandAdmissionResolved`
- To: `Running`

### `AuthorizeSupervisorMobPeerOverlayIdle`
- From: `Idle`
- On: `AuthorizeSupervisorMobPeerOverlay`(supervisor_peer_id, supervisor_epoch, recipient_peer_id, overlay_epoch, endpoints, endpoint_count, command_peer_id, command_endpoint, command_kind)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `local_endpoint_published`
  - `recipient_matches_local_peer`
  - `overlay_endpoint_count_matches`
  - `overlay_peer_ids_unique`
  - `command_peer_id_matches_endpoint`
  - `command_peer_membership_matches_kind`
  - `stale_overlay_epoch`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Idle`

### `AuthorizeSupervisorMobPeerOverlayAttached`
- From: `Attached`
- On: `AuthorizeSupervisorMobPeerOverlay`(supervisor_peer_id, supervisor_epoch, recipient_peer_id, overlay_epoch, endpoints, endpoint_count, command_peer_id, command_endpoint, command_kind)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `local_endpoint_published`
  - `recipient_matches_local_peer`
  - `overlay_endpoint_count_matches`
  - `overlay_peer_ids_unique`
  - `command_peer_id_matches_endpoint`
  - `command_peer_membership_matches_kind`
  - `stale_overlay_epoch`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Attached`

### `AuthorizeSupervisorMobPeerOverlayRunning`
- From: `Running`
- On: `AuthorizeSupervisorMobPeerOverlay`(supervisor_peer_id, supervisor_epoch, recipient_peer_id, overlay_epoch, endpoints, endpoint_count, command_peer_id, command_endpoint, command_kind)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `local_endpoint_published`
  - `recipient_matches_local_peer`
  - `overlay_endpoint_count_matches`
  - `overlay_peer_ids_unique`
  - `command_peer_id_matches_endpoint`
  - `command_peer_membership_matches_kind`
  - `stale_overlay_epoch`
- Emits: `PeerProjectionChanged`, `CommsTrustReconcileRequested`
- To: `Running`

### `RepairSupervisorMobPeerOverlayIdle`
- From: `Idle`
- On: `AuthorizeSupervisorMobPeerOverlay`(supervisor_peer_id, supervisor_epoch, recipient_peer_id, overlay_epoch, endpoints, endpoint_count, command_peer_id, command_endpoint, command_kind)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `local_endpoint_published`
  - `recipient_matches_local_peer`
  - `overlay_endpoint_count_matches`
  - `overlay_peer_ids_unique`
  - `command_peer_id_matches_endpoint`
  - `command_peer_membership_matches_kind`
  - `overlay_epoch_current`
  - `overlay_endpoints_match`
- Emits: `CommsTrustReconcileRequested`
- To: `Idle`

### `RepairSupervisorMobPeerOverlayAttached`
- From: `Attached`
- On: `AuthorizeSupervisorMobPeerOverlay`(supervisor_peer_id, supervisor_epoch, recipient_peer_id, overlay_epoch, endpoints, endpoint_count, command_peer_id, command_endpoint, command_kind)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `local_endpoint_published`
  - `recipient_matches_local_peer`
  - `overlay_endpoint_count_matches`
  - `overlay_peer_ids_unique`
  - `command_peer_id_matches_endpoint`
  - `command_peer_membership_matches_kind`
  - `overlay_epoch_current`
  - `overlay_endpoints_match`
- Emits: `CommsTrustReconcileRequested`
- To: `Attached`

### `RepairSupervisorMobPeerOverlayRunning`
- From: `Running`
- On: `AuthorizeSupervisorMobPeerOverlay`(supervisor_peer_id, supervisor_epoch, recipient_peer_id, overlay_epoch, endpoints, endpoint_count, command_peer_id, command_endpoint, command_kind)
- Guards:
  - `supervisor_bound`
  - `supervisor_peer_id_matches_current`
  - `supervisor_epoch_matches_current`
  - `local_endpoint_published`
  - `recipient_matches_local_peer`
  - `overlay_endpoint_count_matches`
  - `overlay_peer_ids_unique`
  - `command_peer_id_matches_endpoint`
  - `command_peer_membership_matches_kind`
  - `overlay_epoch_current`
  - `overlay_endpoints_match`
- Emits: `CommsTrustReconcileRequested`
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

### `RepairMobPeerOverlayIdle`
- From: `Idle`
- On: `ApplyMobPeerOverlay`(epoch, endpoints)
- Guards:
  - `overlay_epoch_current`
  - `overlay_endpoints_match`
- Emits: `CommsTrustReconcileRequested`
- To: `Idle`

### `RepairMobPeerOverlayAttached`
- From: `Attached`
- On: `ApplyMobPeerOverlay`(epoch, endpoints)
- Guards:
  - `overlay_epoch_current`
  - `overlay_endpoints_match`
- Emits: `CommsTrustReconcileRequested`
- To: `Attached`

### `RepairMobPeerOverlayRunning`
- From: `Running`
- On: `ApplyMobPeerOverlay`(epoch, endpoints)
- Guards:
  - `overlay_epoch_current`
  - `overlay_endpoints_match`
- Emits: `CommsTrustReconcileRequested`
- To: `Running`

### `ClassifyTurnTerminalCauseClassMissingIdle`
- From: `Idle`
- On: `ClassifyTurnTerminalCauseClass`(cause_kind)
- Guards:
  - `cause_missing`
- Emits: `TurnTerminalCauseClassResolved`
- To: `Idle`

### `ClassifyTurnTerminalCauseClassUnknownIdle`
- From: `Idle`
- On: `ClassifyTurnTerminalCauseClass`(cause_kind)
- Guards:
  - `cause_unknown`
- Emits: `TurnTerminalCauseClassResolved`
- To: `Idle`

### `ClassifyTurnTerminalCauseClassBudgetExhaustedIdle`
- From: `Idle`
- On: `ClassifyTurnTerminalCauseClass`(cause_kind)
- Guards:
  - `cause_budget_exhausted`
- Emits: `TurnTerminalCauseClassResolved`
- To: `Idle`

### `ClassifyTurnTerminalCauseClassTimeBudgetExceededIdle`
- From: `Idle`
- On: `ClassifyTurnTerminalCauseClass`(cause_kind)
- Guards:
  - `cause_time_budget_exceeded`
- Emits: `TurnTerminalCauseClassResolved`
- To: `Idle`

### `ClassifyTurnTerminalCauseClassRetryExhaustedIdle`
- From: `Idle`
- On: `ClassifyTurnTerminalCauseClass`(cause_kind)
- Guards:
  - `cause_retry_exhausted`
- Emits: `TurnTerminalCauseClassResolved`
- To: `Idle`

### `ClassifyTurnTerminalCauseClassStructuredOutputValidationFailedIdle`
- From: `Idle`
- On: `ClassifyTurnTerminalCauseClass`(cause_kind)
- Guards:
  - `cause_structured_output_validation_failed`
- Emits: `TurnTerminalCauseClassResolved`
- To: `Idle`

### `ClassifyTurnTerminalCauseClassOtherFailureIdle`
- From: `Idle`
- On: `ClassifyTurnTerminalCauseClass`(cause_kind)
- Guards:
  - `cause_other_failure`
- Emits: `TurnTerminalCauseClassResolved`
- To: `Idle`

### `ClassifyTurnTerminalityTerminalIdle`
- From: `Idle`
- On: `ClassifyTurnTerminality`()
- Guards:
  - `turn_phase_terminal`
- Emits: `TurnTerminalityClassified`
- To: `Idle`

### `ClassifyTurnTerminalityTerminalAttached`
- From: `Attached`
- On: `ClassifyTurnTerminality`()
- Guards:
  - `turn_phase_terminal`
- Emits: `TurnTerminalityClassified`
- To: `Attached`

### `ClassifyTurnTerminalityTerminalRunning`
- From: `Running`
- On: `ClassifyTurnTerminality`()
- Guards:
  - `turn_phase_terminal`
- Emits: `TurnTerminalityClassified`
- To: `Running`

### `ClassifyTurnTerminalityNonTerminalIdle`
- From: `Idle`
- On: `ClassifyTurnTerminality`()
- Guards:
  - `turn_phase_non_terminal`
- Emits: `TurnTerminalityClassified`
- To: `Idle`

### `ClassifyTurnTerminalityNonTerminalAttached`
- From: `Attached`
- On: `ClassifyTurnTerminality`()
- Guards:
  - `turn_phase_non_terminal`
- Emits: `TurnTerminalityClassified`
- To: `Attached`

### `ClassifyTurnTerminalityNonTerminalRunning`
- From: `Running`
- On: `ClassifyTurnTerminality`()
- Guards:
  - `turn_phase_non_terminal`
- Emits: `TurnTerminalityClassified`
- To: `Running`

### `ClassifyLlmFailureRecoveryRecoverIdle`
- From: `Idle`
- On: `ClassifyLlmFailureRecovery`(failure_kind, retry_attempt, max_retries)
- Guards:
  - `failure_kind_recoverable_with_retries`
- Emits: `LlmFailureRecoveryClassified`
- To: `Idle`

### `ClassifyLlmFailureRecoveryRecoverAttached`
- From: `Attached`
- On: `ClassifyLlmFailureRecovery`(failure_kind, retry_attempt, max_retries)
- Guards:
  - `failure_kind_recoverable_with_retries`
- Emits: `LlmFailureRecoveryClassified`
- To: `Attached`

### `ClassifyLlmFailureRecoveryRecoverRunning`
- From: `Running`
- On: `ClassifyLlmFailureRecovery`(failure_kind, retry_attempt, max_retries)
- Guards:
  - `failure_kind_recoverable_with_retries`
- Emits: `LlmFailureRecoveryClassified`
- To: `Running`

### `ClassifyLlmFailureRecoveryExhaustedIdle`
- From: `Idle`
- On: `ClassifyLlmFailureRecovery`(failure_kind, retry_attempt, max_retries)
- Guards:
  - `failure_kind_recoverable_retries_exhausted`
- Emits: `LlmFailureRecoveryClassified`
- To: `Idle`

### `ClassifyLlmFailureRecoveryExhaustedAttached`
- From: `Attached`
- On: `ClassifyLlmFailureRecovery`(failure_kind, retry_attempt, max_retries)
- Guards:
  - `failure_kind_recoverable_retries_exhausted`
- Emits: `LlmFailureRecoveryClassified`
- To: `Attached`

### `ClassifyLlmFailureRecoveryExhaustedRunning`
- From: `Running`
- On: `ClassifyLlmFailureRecovery`(failure_kind, retry_attempt, max_retries)
- Guards:
  - `failure_kind_recoverable_retries_exhausted`
- Emits: `LlmFailureRecoveryClassified`
- To: `Running`

### `ClassifyLlmFailureRecoveryFatalIdle`
- From: `Idle`
- On: `ClassifyLlmFailureRecovery`(failure_kind, retry_attempt, max_retries)
- Guards:
  - `failure_kind_not_recoverable`
- Emits: `LlmFailureRecoveryClassified`
- To: `Idle`

### `ClassifyLlmFailureRecoveryFatalAttached`
- From: `Attached`
- On: `ClassifyLlmFailureRecovery`(failure_kind, retry_attempt, max_retries)
- Guards:
  - `failure_kind_not_recoverable`
- Emits: `LlmFailureRecoveryClassified`
- To: `Attached`

### `ClassifyLlmFailureRecoveryFatalRunning`
- From: `Running`
- On: `ClassifyLlmFailureRecovery`(failure_kind, retry_attempt, max_retries)
- Guards:
  - `failure_kind_not_recoverable`
- Emits: `LlmFailureRecoveryClassified`
- To: `Running`

### `ClassifyAssistantOutputEmptyTerminalIdle`
- From: `Idle`
- On: `ClassifyAssistantOutput`(has_visible_or_actionable)
- Guards:
  - `no_visible_or_actionable_output`
- Emits: `AssistantOutputClassified`
- To: `Idle`

### `ClassifyAssistantOutputEmptyTerminalAttached`
- From: `Attached`
- On: `ClassifyAssistantOutput`(has_visible_or_actionable)
- Guards:
  - `no_visible_or_actionable_output`
- Emits: `AssistantOutputClassified`
- To: `Attached`

### `ClassifyAssistantOutputEmptyTerminalRunning`
- From: `Running`
- On: `ClassifyAssistantOutput`(has_visible_or_actionable)
- Guards:
  - `no_visible_or_actionable_output`
- Emits: `AssistantOutputClassified`
- To: `Running`

### `ClassifyAssistantOutputProceedIdle`
- From: `Idle`
- On: `ClassifyAssistantOutput`(has_visible_or_actionable)
- Guards:
  - `has_visible_or_actionable_output`
- Emits: `AssistantOutputClassified`
- To: `Idle`

### `ClassifyAssistantOutputProceedAttached`
- From: `Attached`
- On: `ClassifyAssistantOutput`(has_visible_or_actionable)
- Guards:
  - `has_visible_or_actionable_output`
- Emits: `AssistantOutputClassified`
- To: `Attached`

### `ClassifyAssistantOutputProceedRunning`
- From: `Running`
- On: `ClassifyAssistantOutput`(has_visible_or_actionable)
- Guards:
  - `has_visible_or_actionable_output`
- Emits: `AssistantOutputClassified`
- To: `Running`

### `ClassifyCallTimeoutRetryableIdle`
- From: `Idle`
- On: `ClassifyCallTimeout`(source, timeout_ms)
- Guards:
  - `call_budget_source`
- Emits: `CallTimeoutClassified`
- To: `Idle`

### `ClassifyCallTimeoutRetryableAttached`
- From: `Attached`
- On: `ClassifyCallTimeout`(source, timeout_ms)
- Guards:
  - `call_budget_source`
- Emits: `CallTimeoutClassified`
- To: `Attached`

### `ClassifyCallTimeoutRetryableRunning`
- From: `Running`
- On: `ClassifyCallTimeout`(source, timeout_ms)
- Guards:
  - `call_budget_source`
- Emits: `CallTimeoutClassified`
- To: `Running`

### `ClassifyCallTimeoutTerminalIdle`
- From: `Idle`
- On: `ClassifyCallTimeout`(source, timeout_ms)
- Guards:
  - `turn_budget_source`
- Emits: `CallTimeoutClassified`
- To: `Idle`

### `ClassifyCallTimeoutTerminalAttached`
- From: `Attached`
- On: `ClassifyCallTimeout`(source, timeout_ms)
- Guards:
  - `turn_budget_source`
- Emits: `CallTimeoutClassified`
- To: `Attached`

### `ClassifyCallTimeoutTerminalRunning`
- From: `Running`
- On: `ClassifyCallTimeout`(source, timeout_ms)
- Guards:
  - `turn_budget_source`
- Emits: `CallTimeoutClassified`
- To: `Running`

### `ResolveTurnSurfaceResultNoneMissingTerminalIdle`
- From: `Idle`
- On: `ResolveTurnSurfaceResult`(outcome, cause_class)
- Guards:
  - `outcome_none`
- Emits: `TurnSurfaceResultResolved`
- To: `Idle`

### `ResolveTurnSurfaceResultCompletedSuccessIdle`
- From: `Idle`
- On: `ResolveTurnSurfaceResult`(outcome, cause_class)
- Guards:
  - `outcome_completed`
  - `cause_absent_or_unknown`
- Emits: `TurnSurfaceResultResolved`
- To: `Idle`

### `ResolveTurnSurfaceResultCompletedFailureIdle`
- From: `Idle`
- On: `ResolveTurnSurfaceResult`(outcome, cause_class)
- Guards:
  - `outcome_completed`
  - `cause_specific_failure`
- Emits: `TurnSurfaceResultResolved`
- To: `Idle`

### `ResolveTurnSurfaceResultFailedHardFailureIdle`
- From: `Idle`
- On: `ResolveTurnSurfaceResult`(outcome, cause_class)
- Guards:
  - `outcome_failed`
  - `cause_any`
- Emits: `TurnSurfaceResultResolved`
- To: `Idle`

### `ResolveTurnSurfaceResultCancelledCancelledIdle`
- From: `Idle`
- On: `ResolveTurnSurfaceResult`(outcome, cause_class)
- Guards:
  - `outcome_cancelled`
  - `cause_absent_or_unknown`
- Emits: `TurnSurfaceResultResolved`
- To: `Idle`

### `ResolveTurnSurfaceResultCancelledFailureIdle`
- From: `Idle`
- On: `ResolveTurnSurfaceResult`(outcome, cause_class)
- Guards:
  - `outcome_cancelled`
  - `cause_specific_failure`
- Emits: `TurnSurfaceResultResolved`
- To: `Idle`

### `ResolveTurnSurfaceResultBudgetExhaustedSuccessIdle`
- From: `Idle`
- On: `ResolveTurnSurfaceResult`(outcome, cause_class)
- Guards:
  - `outcome_budget_exhausted`
  - `cause_budget_exhausted`
- Emits: `TurnSurfaceResultResolved`
- To: `Idle`

### `ResolveTurnSurfaceResultBudgetExhaustedFailureIdle`
- From: `Idle`
- On: `ResolveTurnSurfaceResult`(outcome, cause_class)
- Guards:
  - `outcome_budget_exhausted`
  - `cause_not_budget_exhausted`
- Emits: `TurnSurfaceResultResolved`
- To: `Idle`

### `ResolveTurnSurfaceResultTimeBudgetExceededHardFailureIdle`
- From: `Idle`
- On: `ResolveTurnSurfaceResult`(outcome, cause_class)
- Guards:
  - `outcome_time_budget_exceeded`
  - `cause_any`
- Emits: `TurnSurfaceResultResolved`
- To: `Idle`

### `ResolveTurnSurfaceResultStructuredOutputValidationFailedHardFailureIdle`
- From: `Idle`
- On: `ResolveTurnSurfaceResult`(outcome, cause_class)
- Guards:
  - `outcome_structured_output_validation_failed`
  - `cause_any`
- Emits: `TurnSurfaceResultResolved`
- To: `Idle`

## Coverage
### Code Anchors
- `meerkat_machine` (machine `MeerkatMachine`): `meerkat-runtime/src/meerkat_machine/mod.rs` — authoritative MeerkatMachine command dispatch and state ownership for initialize, recover initializing, register, unregister, deferred session stage, deferred session keep-alive update, deferred session promotion, deferred session archive, deferred session drop, mob operator access resolution/restoration/profile mutation/create scope/manage scope/spawn-profile scope, reconfigure, stage filters and tools, prepare bindings, drain, interrupt, cancel boundary, cancellation, abort, wait, ingest, publish event, accept input, recover input lifecycle, classify input terminality, classify envelope, append/context starts, run preparation, primitive applied conversation/immediate, enter extraction, extraction validation passed/failed retry/exhausted, recoverable/fatal failure, retry requested, budget exhausted, steer accepted, increment attempt count, rollback staged, consume on accept, commit, fail, pending/call/finalize tool surface, retire/retired, reset, stop/stopped executor, destroy/destroyed, ensure executor, runtime notice, silent intents, recycle, realtime binding, MCP server, peer ready operation, peer request, peer response, peer ingress, peer endpoint projection, interaction stream, product turn, live topology, ingress, supervisor, trust reconcile, ops barrier, local endpoint, admission, completion, completion consumer cursors, compaction, submit op event, progress reported op, terminate op, resolve op lifecycle transition rejected feedback, notify op watcher, recover op record, classify operation terminality, classify recovered operation record, recover ops completion cursor, recover/advance completion consumer cursors, evict completed op, collect completed op, collect/enqueue, terminal records, model routing status, set model routing baseline, finite switch turn, until changed switch turn, assistant turn admission, image operation begin activate complete restore, routing approval, routing denial, scoped override, sync visibility revisions, and persistent reconfigure
- `meerkat_public_surface` (machine `MeerkatMachine`): `meerkat/src/meerkat_machine.rs` — MeerkatMachine snapshot/diagnostic facade

### Scenarios
- `bind-run-boundary-terminal` — runtime binds, runs work, applies a boundary, and reports a terminal outcome
- `retire-reset-destroy` — runtime retires, resets, stops, and destroys without reopening superseded work
- `staged_visibility_apply` — tool visibility staged state promotes into the committed visible revision at a boundary
- `turn_interrupt_and_shutdown` — running work records interrupt and shutdown intent without escaping the Meerkat authority boundary
- `session_registration_and_binding` — initialize, recover initializing, register, unregister, deferred session stage, keep-alive update, promotion, archive, drop, mob operator access resolve/restore/scope mutation, reconfigure session identity, prepare bindings, ensure executor, attach session ingress, detach ingress, drain exit, and runtime bound/retired/destroyed notices
- `input_admission_and_queueing` — ingest and publish event, accept input with or without completion, classify input terminality, classify external envelope or plain event, classify peer message, peer request, peer response, and peer ingress, prepare run work, primitive applied conversation or immediate, enter extraction, extraction validation passed, recoverable or fatal failure, budget exhausted, steer accepted, increment attempt count, consume on accept, enqueue classified entry, resolve admission, submit admitted ingress effect, post admission signal, and input or ingress notices
- `ops_completion_and_waiters` — abort, wait, abort all, peer ready operation, request cancellation at boundary, completion produced/resolved, wait all satisfied, collect completed result, recover op record, classify operation terminality, classify recovered operation record, recover ops completion cursor, recover/advance completion consumer cursors, evict completed op, collect completed op, submit op event, resolve op lifecycle transition rejected feedback, notify op watcher, reject surface call, retain discard or evict completed terminal records
- `realtime_connection_projection` — project realtime intent, begin replace detach binding, require reattach, publish signal, reconnect progress, MCP server connect/connected/failed/disconnected/reload, advance session context, interaction stream reserved/attached/completed/expired/closed early, freshness, policy, and binding rotation
- `product_turn_streaming` — product turn in flight, committed, output started, interrupted, terminal, realtime projection advance/refreshed/reset, client input submitted, mid turn activity, and turn terminated classification
- `recycle_and_compaction` — recycle from idle or retired, initiate recycle, check compaction, and re-enter ready runtime ownership without preserving stale completed records
- `model_routing_and_image_operation` — set model routing baseline, request finite switch turn, request until changed switch turn, admit model routing assistant turn, begin image operation, activate image operation override, complete image operation, restore image operation override, project model routing status changed, switch turn denied, switch turn persistent reconfigure requested, switch turn finite override activated/restored, image operation phase changed/denied, and model routing approval terminalized
- `live_topology_and_supervision` — begin live topology reconfigure, mark detached, apply identity or visibility, complete/abort/fail topology, bind/authorize/revoke supervisor, publish/revoke trust edge, comms trust reconcile, and local endpoint publish or clear
