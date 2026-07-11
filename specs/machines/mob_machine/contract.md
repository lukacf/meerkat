# MobMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `2`
- Rust owner: `self` / `catalog::dsl::mob_machine`

## State
- Phase enum: `Running | Stopped | Completed | Destroyed`
- `destroy_admitted`: `Bool`
- `live_runtime_ids`: `Set<AgentRuntimeId>`
- `externally_addressable_runtime_ids`: `Set<AgentRuntimeId>`
- `runtime_fence_tokens`: `Map<AgentRuntimeId, FenceToken>`
- `identity_runtime_generations`: `Map<AgentIdentity, Generation>`
- `identity_runtime_fence_tokens`: `Map<AgentIdentity, FenceToken>`
- `active_run_count`: `u64`
- `flow_authority_schema_version`: `u64`
- `run_status`: `Map<RunId, FlowRunStatus>`
- `run_ordered_steps`: `Map<RunId, Seq<StepId>>`
- `run_tracked_steps`: `Map<RunId, Set<StepId>>`
- `run_step_status`: `Map<RunId, Map<StepId, Option<StepRunStatus>>>`
- `run_output_recorded`: `Map<RunId, Map<StepId, Bool>>`
- `run_step_condition_results`: `Map<RunId, Map<StepId, Option<Bool>>>`
- `run_step_has_conditions`: `Map<RunId, Map<StepId, Bool>>`
- `run_step_dependencies`: `Map<RunId, Map<StepId, Seq<StepId>>>`
- `run_step_dependency_modes`: `Map<RunId, Map<StepId, DependencyMode>>`
- `run_step_branches`: `Map<RunId, Map<StepId, Option<BranchId>>>`
- `run_step_collection_policies`: `Map<RunId, Map<StepId, CollectionPolicyKind>>`
- `run_step_quorum_thresholds`: `Map<RunId, Map<StepId, u32>>`
- `run_step_target_counts`: `Map<RunId, Map<StepId, u64>>`
- `run_step_target_success_counts`: `Map<RunId, Map<StepId, u64>>`
- `run_step_target_terminal_failure_counts`: `Map<RunId, Map<StepId, u64>>`
- `run_target_retry_counts`: `Map<RunId, Map<String, u64>>`
- `run_failure_count`: `Map<RunId, u64>`
- `run_consecutive_failure_count`: `Map<RunId, u64>`
- `run_escalation_threshold`: `Map<RunId, u64>`
- `run_max_step_retries`: `Map<RunId, u32>`
- `run_ready_frames`: `Map<RunId, Seq<FrameId>>`
- `run_ready_frame_membership`: `Map<RunId, Set<FrameId>>`
- `run_ready_frame_membership_flat`: `Set<FrameId>`
- `run_pending_body_frame_loops`: `Map<RunId, Seq<LoopInstanceId>>`
- `run_pending_body_frame_loop_membership`: `Map<RunId, Set<LoopInstanceId>>`
- `run_pending_body_frame_loop_membership_flat`: `Set<LoopInstanceId>`
- `run_active_node_count`: `Map<RunId, u64>`
- `run_active_frame_count`: `Map<RunId, u64>`
- `run_last_granted_frame`: `Map<RunId, Option<FrameId>>`
- `run_last_granted_loop`: `Map<RunId, Option<LoopInstanceId>>`
- `run_max_active_nodes`: `Map<RunId, u64>`
- `run_max_active_frames`: `Map<RunId, u64>`
- `run_max_frame_depth`: `Map<RunId, u64>`
- `frame_scope`: `Map<FrameId, FrameScope>`
- `frame_phase`: `Map<FrameId, FrameStatus>`
- `frame_run`: `Map<FrameId, RunId>`
- `frame_parent_loop`: `Map<FrameId, Option<LoopInstanceId>>`
- `frame_iteration`: `Map<FrameId, u32>`
- `frame_tracked_nodes`: `Map<FrameId, Set<FlowNodeId>>`
- `frame_ordered_nodes`: `Map<FrameId, Seq<FlowNodeId>>`
- `frame_node_kind`: `Map<FrameId, Map<FlowNodeId, FlowNodeKind>>`
- `frame_node_dependencies`: `Map<FrameId, Map<FlowNodeId, Seq<FlowNodeId>>>`
- `frame_node_dependency_modes`: `Map<FrameId, Map<FlowNodeId, DependencyMode>>`
- `frame_node_step_ids`: `Map<FrameId, Map<FlowNodeId, StepId>>`
- `frame_node_loop_ids`: `Map<FrameId, Map<FlowNodeId, LoopId>>`
- `frame_node_status`: `Map<FrameId, Map<FlowNodeId, NodeRunStatus>>`
- `frame_ready_queue`: `Map<FrameId, Seq<FlowNodeId>>`
- `frame_output_recorded`: `Map<FrameId, Map<FlowNodeId, Bool>>`
- `frame_last_admitted_node`: `Map<FrameId, Option<FlowNodeId>>`
- `frame_node_condition_results`: `Map<FrameId, Map<FlowNodeId, Option<Bool>>>`
- `frame_node_branches`: `Map<FrameId, Map<FlowNodeId, Option<BranchId>>>`
- `loop_phase`: `Map<LoopInstanceId, LoopStatus>`
- `loop_parent_frame`: `Map<LoopInstanceId, FrameId>`
- `loop_parent_node`: `Map<LoopInstanceId, FlowNodeId>`
- `loop_definition`: `Map<LoopInstanceId, LoopId>`
- `loop_depth`: `Map<LoopInstanceId, u32>`
- `loop_stage`: `Map<LoopInstanceId, LoopIterationStage>`
- `loop_current_iteration`: `Map<LoopInstanceId, u64>`
- `loop_last_completed_iteration`: `Map<LoopInstanceId, u64>`
- `loop_max_iterations`: `Map<LoopInstanceId, u64>`
- `loop_active_body_frame`: `Map<LoopInstanceId, Option<FrameId>>`
- `pending_spawn_count`: `u64`
- `pending_spawn_sessions`: `Map<AgentIdentity, SessionId>`
- `coordinator_bound`: `Bool`
- `member_startup_binding_requested`: `Set<AgentRuntimeId>`
- `member_startup_runtime_ready`: `Set<AgentRuntimeId>`
- `member_startup_ready`: `Set<AgentRuntimeId>`
- `member_kickoff_pending`: `Set<AgentIdentity>`
- `member_kickoff_starting`: `Set<AgentIdentity>`
- `member_kickoff_callback_pending`: `Set<AgentIdentity>`
- `member_kickoff_started`: `Set<AgentIdentity>`
- `member_kickoff_failed`: `Set<AgentIdentity>`
- `member_kickoff_cancelled`: `Set<AgentIdentity>`
- `member_kickoff_error`: `Map<AgentIdentity, String>`
- `member_restore_failures`: `Map<AgentIdentity, String>`
- `member_restore_failure_codes`: `Map<AgentIdentity, String>`
- `runtime_retire_refusal_codes`: `Map<AgentRuntimeId, String>`
- `runtime_retire_refusal_reasons`: `Map<AgentRuntimeId, String>`
- `runtime_retire_pending_sessions`: `Map<AgentRuntimeId, SessionId>`
- `remote_runtime_retired_ids`: `Set<AgentRuntimeId>`
- `remote_supervisor_revoked_ids`: `Set<AgentRuntimeId>`
- `member_revival_pending`: `Set<AgentIdentity>`
- `spawn_exec_phase`: `Map<AgentIdentity, SpawnExecPhase>`
- `member_state_markers`: `Map<AgentRuntimeId, MobMemberState>`
- `wiring_edges`: `Set<WiringEdge>`
- `external_peer_edges`: `Set<ExternalPeerEdge>`
- `external_peer_edges_by_key`: `Map<ExternalPeerKey, ExternalPeerEdge>`
- `supervisor_authority_peer_id`: `Option<PeerId>`
- `supervisor_authority_signing_key`: `Option<PeerSigningKey>`
- `supervisor_authority_epoch`: `Option<u64>`
- `supervisor_authority_protocol_version`: `Option<SupervisorProtocolVersion>`
- `supervisor_pending_authority_peer_id`: `Option<PeerId>`
- `supervisor_pending_authority_signing_key`: `Option<PeerSigningKey>`
- `supervisor_pending_authority_epoch`: `Option<u64>`
- `supervisor_pending_authority_protocol_version`: `Option<SupervisorProtocolVersion>`
- `supervisor_pending_authority_operation_id`: `Option<String>`
- `supervisor_pending_authority_accepted_peer_ids`: `Set<PeerId>`
- `supervisor_pending_authority_member_target_names`: `Map<PeerId, String>`
- `supervisor_pending_authority_member_target_addresses`: `Map<PeerId, String>`
- `pending_recipient_trust`: `Set<PeerId>`
- `owner_bridge_session_id`: `Option<SessionId>`
- `owner_bridge_destroy_on_archive`: `Bool`
- `implicit_delegation_mob`: `Bool`
- `identity_to_runtime`: `Map<AgentIdentity, AgentRuntimeId>`
- `member_session_bindings`: `Map<AgentIdentity, SessionId>`
- `member_profile_names`: `Map<AgentIdentity, String>`
- `member_runtime_modes`: `Map<AgentIdentity, SpawnPolicyRuntimeMode>`
- `member_peer_ids`: `Map<AgentIdentity, PeerId>`
- `member_peer_endpoints`: `Map<AgentIdentity, MemberPeerEndpoint>`
- `member_prior_peer_endpoints`: `Map<AgentIdentity, Set<MemberPeerEndpoint>>`
- `pending_session_ingress_detach_runtime_ids`: `Set<AgentRuntimeId>`
- `spawn_policy_enabled`: `Bool`
- `spawn_policy_revision`: `u64`
- `spawn_policy_resolution_revision`: `Map<AgentIdentity, u64>`
- `spawn_policy_resolution_profiles`: `Map<AgentIdentity, String>`
- `spawn_policy_resolution_runtime_modes`: `Map<AgentIdentity, Option<SpawnPolicyRuntimeMode>>`
- `spawn_policy_resolution_absent`: `Set<AgentIdentity>`
- `spawn_profile_authority_profile_names`: `Map<AgentIdentity, String>`
- `spawn_profile_authority_models`: `Map<AgentIdentity, String>`
- `spawn_profile_authority_material_digests`: `Map<AgentIdentity, String>`
- `spawn_profile_authority_tool_config_digests`: `Map<AgentIdentity, String>`
- `spawn_profile_authority_skills_digests`: `Map<AgentIdentity, String>`
- `spawn_profile_authority_provider_params_digests`: `Map<AgentIdentity, Option<String>>`
- `spawn_profile_authority_output_schema_digests`: `Map<AgentIdentity, Option<String>>`
- `spawn_profile_authority_external_addressable`: `Map<AgentIdentity, Bool>`
- `topology_epoch`: `u64`
- `work_intent_status`: `Map<WorkIntentId, MobCoordinationWorkIntentStatus>`
- `work_intent_revision`: `Map<WorkIntentId, u64>`
- `work_intent_resources`: `Map<WorkIntentId, Set<CoordinationResourceRef>>`
- `work_intent_owner_present`: `Map<WorkIntentId, Bool>`
- `work_intent_expires_at_ms`: `Map<WorkIntentId, Option<u64>>`
- `resource_claim_status`: `Map<ResourceClaimId, MobCoordinationResourceClaimStatus>`
- `resource_claim_kind`: `Map<ResourceClaimId, MobCoordinationResourceClaimKind>`
- `resource_claim_revision`: `Map<ResourceClaimId, u64>`
- `resource_claim_resources`: `Map<ResourceClaimId, Set<CoordinationResourceRef>>`
- `resource_claim_owner_present`: `Map<ResourceClaimId, Bool>`
- `resource_claim_expires_at_ms`: `Map<ResourceClaimId, Option<u64>>`
- `coordination_event_next_sequence`: `u64`
- `topology_default_policy`: `PolicyDecision`
- `external_member_rebind_capability`: `Map<AgentIdentity, ExternalMemberRebindCapability>`
- `orphan_budget`: `u64`
- `desired_members`: `Set<AgentIdentity>`
- `members_to_spawn`: `Set<AgentIdentity>`
- `members_to_retire`: `Set<AgentIdentity>`
- `adaptive_active_run`: `Option<AdaptiveRunId>`
- `adaptive_run_phase`: `Map<AdaptiveRunId, AdaptiveRunPhase>`
- `adaptive_stop_reason`: `Map<AdaptiveRunId, AdaptiveStopReason>`
- `adaptive_limit_max_depth`: `Map<AdaptiveRunId, u64>`
- `adaptive_limit_max_total_decisions`: `Map<AdaptiveRunId, u64>`
- `adaptive_limit_max_repair_attempts`: `Map<AdaptiveRunId, u64>`
- `adaptive_limit_max_layer_failures`: `Map<AdaptiveRunId, u64>`
- `adaptive_limit_max_attempts_per_layer`: `Map<AdaptiveRunId, u64>`
- `adaptive_limit_max_members_per_layer`: `Map<AdaptiveRunId, u64>`
- `adaptive_limit_max_total_spawned_members`: `Map<AdaptiveRunId, u64>`
- `adaptive_limit_max_active_members`: `Map<AdaptiveRunId, u64>`
- `adaptive_limit_max_retained_layer_mobs`: `Map<AdaptiveRunId, u64>`
- `adaptive_limit_max_aggregate_tokens`: `Map<AdaptiveRunId, u64>`
- `adaptive_limit_max_aggregate_tool_calls`: `Map<AdaptiveRunId, u64>`
- `adaptive_limit_allowed_model_classes`: `Map<AdaptiveRunId, Set<String>>`
- `adaptive_limit_allowed_tool_classes`: `Map<AdaptiveRunId, Set<String>>`
- `adaptive_limit_allowed_skill_identities`: `Map<AdaptiveRunId, Set<String>>`
- `adaptive_limit_allowed_auth_binding_refs`: `Map<AdaptiveRunId, Set<String>>`
- `adaptive_deadline_ms`: `Map<AdaptiveRunId, u64>`
- `adaptive_depth`: `Map<AdaptiveRunId, u64>`
- `adaptive_total_decisions`: `Map<AdaptiveRunId, u64>`
- `adaptive_repair_attempts`: `Map<AdaptiveRunId, u64>`
- `adaptive_layer_failures`: `Map<AdaptiveRunId, u64>`
- `adaptive_total_spawned_members`: `Map<AdaptiveRunId, u64>`
- `adaptive_active_members`: `Map<AdaptiveRunId, u64>`
- `adaptive_retained_layer_mobs`: `Map<AdaptiveRunId, u64>`
- `adaptive_aggregate_token_reserved`: `Map<AdaptiveRunId, u64>`
- `adaptive_aggregate_token_actual`: `Map<AdaptiveRunId, u64>`
- `adaptive_aggregate_tool_call_reserved`: `Map<AdaptiveRunId, u64>`
- `adaptive_aggregate_tool_call_actual`: `Map<AdaptiveRunId, u64>`
- `adaptive_active_layer`: `Map<AdaptiveRunId, AdaptiveLayerId>`
- `adaptive_layer_adaptive_run`: `Map<AdaptiveLayerId, AdaptiveRunId>`
- `adaptive_layer_phase`: `Map<AdaptiveLayerId, AdaptiveLayerPhase>`
- `adaptive_layer_attempt`: `Map<AdaptiveLayerId, u64>`
- `adaptive_layer_member_count`: `Map<AdaptiveLayerId, u64>`
- `adaptive_layer_plan_digest`: `Map<AdaptiveLayerId, String>`
- `adaptive_layer_child_mob_id`: `Map<AdaptiveLayerId, MobId>`
- `adaptive_layer_token_reservation`: `Map<AdaptiveLayerId, u64>`
- `adaptive_layer_tool_call_reservation`: `Map<AdaptiveLayerId, u64>`
- `adaptive_layer_run_id`: `Map<AdaptiveLayerId, RunId>`
- `adaptive_layer_result_digest`: `Map<AdaptiveLayerId, String>`
- `adaptive_layer_fault`: `Map<AdaptiveLayerId, AdaptiveLayerSetupFaultKind>`
- `adaptive_layer_disposition`: `Map<AdaptiveLayerId, AdaptiveLayerDispositionKind>`
- `adaptive_missing_body_digest`: `Map<AdaptiveRunId, String>`

## Inputs
- `RunFlow`(run_id: RunId, step_ids: Set<StepId>, ordered_steps: Seq<StepId>, step_status: Map<StepId, Option<StepRunStatus>>, output_recorded: Map<StepId, Bool>, step_condition_results: Map<StepId, Option<Bool>>, step_has_conditions: Map<StepId, Bool>, step_dependencies: Map<StepId, Seq<StepId>>, step_dependency_modes: Map<StepId, DependencyMode>, step_branches: Map<StepId, Option<BranchId>>, step_collection_policies: Map<StepId, CollectionPolicyKind>, step_quorum_thresholds: Map<StepId, u32>, step_target_counts: Map<StepId, u64>, step_target_success_counts: Map<StepId, u64>, step_target_terminal_failure_counts: Map<StepId, u64>, escalation_threshold: u64, max_step_retries: u32, max_active_nodes: u64, max_active_frames: u64, max_frame_depth: u64)
- `CancelFlow`(run_id: RunId)
- `FlowStatus`
- `CommitSpawnMembership`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, profile_material_digest: String, external_addressable: Bool, runtime_mode: SpawnPolicyRuntimeMode, bridge_session_id: Option<SessionId>, replacing: Option<SessionId>)
- `EnsureMember`(agent_identity: AgentIdentity)
- `Reconcile`(desired: Set<AgentIdentity>, retire_stale: Bool)
- `Retire`(mob_id: MobId, agent_runtime_id: AgentRuntimeId, agent_identity: AgentIdentity, generation: Generation, releasing: Option<SessionId>, session_id: Option<SessionId>)
- `Respawn`(agent_runtime_id: AgentRuntimeId)
- `RetireAll`
- `WireMembers`(edge: WiringEdge)
- `WireMembersWithTrust`(edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity)
- `UnwireMembers`(edge: WiringEdge)
- `WireExternalPeer`(key: ExternalPeerKey, edge: ExternalPeerEdge)
- `UnwireExternalPeer`(key: ExternalPeerKey, edge: ExternalPeerEdge)
- `SubmitWork`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId, origin: WorkOrigin)
- `CancelWork`(work_id: WorkId)
- `CancelAllWork`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `Stop`
- `Resume`
- `Complete`
- `Reset`
- `Destroy`
- `RosterSnapshot`
- `ListMembers`
- `ListMembersIncludingRetiring`
- `ListAllMembers`
- `MemberStatus`
- `SubscribeAgentEvents`(agent_identity: AgentIdentity)
- `SubscribeAllAgentEvents`(session_bound_runtimes: Set<AgentRuntimeId>)
- `SubscribeMobEvents`(initial_cursor: u64, channel_capacity: u64, poll_interval_ms: u64, session_bound_runtimes: Set<AgentRuntimeId>)
- `PollEvents`
- `ReplayAllEvents`
- `RecordOperatorActionProvenance`(tool_name: String, principal_token: OpaquePrincipalToken, caller_provenance: Option<MobToolCallerProvenance>, audit_invocation_id: Option<String>)
- `GetMember`
- `SetSpawnPolicy`(enabled: Bool)
- `Shutdown`
- `ForceCancel`(agent_identity: AgentIdentity)

## Surface-only Inputs
- `FlowStatus`
- `RosterSnapshot`
- `ListMembers`
- `ListMembersIncludingRetiring`
- `ListAllMembers`
- `MemberStatus`
- `CancelWork`
- `PollEvents`
- `ReplayAllEvents`
- `GetMember`

## Runtime-Internal Inputs
- `CreateRunSeed`(run_id: RunId, step_ids: Set<StepId>, ordered_steps: Seq<StepId>, step_status: Map<StepId, Option<StepRunStatus>>, output_recorded: Map<StepId, Bool>, step_condition_results: Map<StepId, Option<Bool>>, step_has_conditions: Map<StepId, Bool>, step_dependencies: Map<StepId, Seq<StepId>>, step_dependency_modes: Map<StepId, DependencyMode>, step_branches: Map<StepId, Option<BranchId>>, step_collection_policies: Map<StepId, CollectionPolicyKind>, step_quorum_thresholds: Map<StepId, u32>, step_target_counts: Map<StepId, u64>, step_target_success_counts: Map<StepId, u64>, step_target_terminal_failure_counts: Map<StepId, u64>, escalation_threshold: u64, max_step_retries: u32, max_active_nodes: u64, max_active_frames: u64, max_frame_depth: u64)
- `CreateFrameSeed`(run_id: RunId, frame_id: FrameId, frame_scope: FrameScope, loop_instance_id: Option<LoopInstanceId>, iteration: u32, tracked_nodes: Set<FlowNodeId>, ordered_nodes: Seq<FlowNodeId>, node_kind: Map<FlowNodeId, FlowNodeKind>, node_dependencies: Map<FlowNodeId, Seq<FlowNodeId>>, node_dependency_modes: Map<FlowNodeId, DependencyMode>, node_branches: Map<FlowNodeId, Option<BranchId>>, node_step_ids: Map<FlowNodeId, StepId>, node_loop_ids: Map<FlowNodeId, LoopId>, node_status: Map<FlowNodeId, NodeRunStatus>, ready_queue: Seq<FlowNodeId>, output_recorded: Map<FlowNodeId, Bool>, node_condition_results: Map<FlowNodeId, Option<Bool>>, last_admitted_node: Option<FlowNodeId>)
- `CreateLoopSeed`(loop_instance_id: LoopInstanceId, parent_frame_id: FrameId, parent_node_id: FlowNodeId, loop_id: LoopId, depth: u32, max_iterations: u64)
- `RecordLoopBodyFrameCompleted`(loop_instance_id: LoopInstanceId, iteration: u64)
- `RecordLoopUntilConditionMet`(loop_instance_id: LoopInstanceId, iteration: u64)
- `RecordLoopUntilConditionFailed`(loop_instance_id: LoopInstanceId, iteration: u64)
- `AuthorizeFlowRunReducerCommand`(run_id: RunId, command: FlowRunReducerCommandKind, step_id: Option<StepId>, step_status: Option<StepRunStatus>, target_count: Option<u64>, frame_id: Option<FrameId>, node_id: Option<FlowNodeId>, loop_instance_id: Option<LoopInstanceId>, retry_key: Option<String>)
- `AuthorizeFlowFrameReducerCommand`(frame_id: FrameId, command: FlowFrameReducerCommandKind, node_id: Option<FlowNodeId>, node_status: Option<NodeRunStatus>, terminal_status: Option<FrameStatus>)
- `AuthorizeLoopIterationReducerCommand`(loop_instance_id: LoopInstanceId, command: LoopIterationReducerCommandKind, body_frame_id: Option<FrameId>, body_frame_iteration: Option<u64>)
- `ClassifyFlowRunTerminality`(run_id: RunId, status: FlowRunStatus)
- `ClassifyFlowStepTerminality`(run_id: RunId, step_id: StepId, status: StepRunStatus)
- `ClassifyFlowFrameTerminalStatus`(frame_id: FrameId)
- `ClassifyFlowRunPublicResult`(run_id: RunId, status: FlowRunStatus)
- `BeginSpawnExec`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, profile_material_digest: String, external_addressable: Bool, runtime_mode: SpawnPolicyRuntimeMode, bridge_session_id: Option<SessionId>, replacing: Option<SessionId>)
- `CommitSpawnActivation`(agent_identity: AgentIdentity)
- `AbortSpawnExec`(agent_identity: AgentIdentity)
- `AuthorizeSpawnProfile`(agent_identity: AgentIdentity, profile_name: String, model: String, profile_material_digest: String, tool_config_digest: String, skills_digest: String, provider_params_digest: Option<String>, output_schema_digest: Option<String>, external_addressable: Bool)
- `ClassifySpawnManyFailure`(observation: MobSpawnManyFailureObservationKind)
- `ClassifyMemberWait`(agent_identity: AgentIdentity)
- `ResolveFlowDelegationEdgeAdmission`(from_role: String, to_role: String, rule_verdict: MobFlowDelegationEdgeRuleVerdictKind, mode: MobFlowDelegationEdgeModeKind)
- `ClassifyRemoteMemberRuntimeObservation`(observed_state: MobRemoteMemberRuntimeObservedState)
- `ResolveSpawnMemberAdmission`(manage_scope_present: Bool, profile_scope_contains: Bool, privileged_resume_bridge_session_present: Bool, privileged_resume_session_present: Bool, privileged_backend_present: Bool, privileged_runtime_mode_present: Bool, privileged_launch_mode_present: Bool, privileged_tool_access_policy_present: Bool, privileged_budget_split_policy_present: Bool, privileged_tooling_present: Bool, privileged_auth_binding_present: Bool)
- `ResolveCurrentMobAdmission`(can_manage_mob: Bool)
- `ResolveSpawnToolAdmission`(can_manage_mob: Bool, spawn_profile_scope_present: Bool)
- `ResolveCreateMobAdmission`(can_create_mobs: Bool)
- `ResolveProfileMutationAdmission`(can_mutate_profiles: Bool)
- `ClassifyMemberOperationEligibility`
- `ClassifyBridgeRejectionRecovery`(rejection_cause: MobBridgeRejectionCause)
- `ClassifyPendingSupervisorAcceptance`(rejection_cause: MobBridgeRejectionCause)
- `RetireAbsent`(agent_identity: AgentIdentity)
- `RequestPendingSessionIngressDetachForMobDestroy`(mob_id: MobId, agent_runtime_id: AgentRuntimeId)
- `BindOwnerBridgeSession`(bridge_session_id: SessionId, destroy_on_owner_archive: Bool, implicit_delegation_mob: Bool)
- `CleanupRetiringMemberWiring`(edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `RestoreRetiringMemberWiring`(edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `RegisterMemberPeer`(agent_identity: AgentIdentity, peer_endpoint: MemberPeerEndpoint)
- `AuthorizeMemberEndpointMigrationTrustCleanup`(edge: WiringEdge, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, retained_peer_endpoint: MemberPeerEndpoint)
- `AuthorizeMemberPeerRebind`(agent_identity: AgentIdentity, expected_peer_endpoint: MemberPeerEndpoint)
- `AuthorizeMemberPeerOverlay`(agent_identity: AgentIdentity, expected_peer_endpoint: MemberPeerEndpoint)
- `AuthorizeMemberTrustWiring`(edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity)
- `AuthorizeMemberTrustUnwiring`(edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity)
- `AuthorizeMemberTrustCleanup`(edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity)
- `AuthorizeMemberTrustCleanupObserved`(edge: WiringEdge, a_identity: AgentIdentity, a_peer_id: PeerId, b_identity: AgentIdentity, b_peer_id: PeerId)
- `AuthorizeRetiringMemberTrustCleanupObserved`(edge: WiringEdge, a_identity: AgentIdentity, a_peer_id: PeerId, b_identity: AgentIdentity, b_peer_id: PeerId, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `AuthorizeExternalPeerReciprocalTrust`(key: ExternalPeerKey, agent_identity: AgentIdentity)
- `CleanupRetiringExternalPeer`(key: ExternalPeerKey, edge: ExternalPeerEdge, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `RestoreRetiringExternalPeer`(key: ExternalPeerKey, edge: ExternalPeerEdge, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `CleanupRetiringExternalPeerObservedAbsent`(key: ExternalPeerKey, edge: ExternalPeerEdge, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `RestoreRetiringExternalPeerObservedAbsent`(key: ExternalPeerKey, edge: ExternalPeerEdge, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `AdmitSupervisorRotation`
- `ProvisionSupervisorAuthority`(peer_id: PeerId, signing_key: PeerSigningKey, epoch: u64, protocol_version: SupervisorProtocolVersion)
- `RecordSupervisorPendingRotation`(current_peer_id: PeerId, current_epoch: u64, current_protocol_version: SupervisorProtocolVersion, operation_id: String, pending_peer_id: PeerId, pending_signing_key: PeerSigningKey, pending_epoch: u64, pending_protocol_version: SupervisorProtocolVersion, accepted_peer_ids: Set<PeerId>, active_peer_ids: Set<PeerId>, member_target_names: Map<PeerId, String>, member_target_addresses: Map<PeerId, String>)
- `CommitSupervisorRotation`(current_peer_id: PeerId, current_epoch: u64, current_protocol_version: SupervisorProtocolVersion, operation_id: String, next_peer_id: PeerId, next_signing_key: PeerSigningKey, next_epoch: u64, next_protocol_version: SupervisorProtocolVersion)
- `ClearSupervisorAuthorityForDestroy`(current_peer_id: PeerId, current_signing_key: PeerSigningKey, current_epoch: u64, protocol_version: SupervisorProtocolVersion)
- `RestoreSupervisorAuthorityAfterDestroyRollback`(peer_id: PeerId, signing_key: PeerSigningKey, epoch: u64, protocol_version: SupervisorProtocolVersion, pending_operation_id: Option<String>, pending_peer_id: Option<PeerId>, pending_signing_key: Option<PeerSigningKey>, pending_epoch: Option<u64>, pending_protocol_version: Option<SupervisorProtocolVersion>, pending_accepted_peer_ids: Set<PeerId>, pending_member_target_names: Map<PeerId, String>, pending_member_target_addresses: Map<PeerId, String>)
- `RecordPendingRecipientTrust`(peer_id: PeerId)
- `ResolvePendingRecipientTrust`(peer_id: PeerId)
- `RollbackPendingRecipientTrust`(peer_id: PeerId)
- `SessionIngressDetachedForMobDestroy`(mob_id: MobId, agent_runtime_id: AgentRuntimeId)
- `SessionIngressDetachFailedForMobDestroy`(mob_id: MobId, agent_runtime_id: AgentRuntimeId, reason: String)
- `ResolveSubmitWorkRejection`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, origin: WorkOrigin)
- `ResolveRuntimeBindingRefusal`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, session_id: SessionId, refusal_code: String, reason: String)
- `ResolveRuntimeIngressRefusal`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, session_id: SessionId, work_id: WorkId, origin: WorkOrigin, refusal_code: String, reason: String)
- `ResolveRuntimeRetireRefusal`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, session_id: SessionId, refusal_code: String, reason: String)
- `RetryRuntimeRetire`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId)
- `RecordRemoteMemberRuntimeRetired`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `RecordRemoteMemberSupervisorRevoked`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `ResolveCancelAllWorkRejection`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `SubscribeStructuralEvents`(after_cursor: u64, latest_cursor: u64, explicit_after_cursor: Bool, batch_limit: u64, channel_capacity: u64)
- `AuthorizeMobEventRouterMemberSubscription`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `AuthorizeMobEventRouterMemberRemoval`(agent_identity: AgentIdentity)
- `PollEventsStrict`(after_cursor: u64, latest_cursor: u64, limit: u64)
- `ResolveSpawnPolicy`(agent_identity: AgentIdentity, revision: u64, profile_name: Option<String>, runtime_mode: Option<SpawnPolicyRuntimeMode>)
- `KickoffMarkPending`(member_id: AgentIdentity)
- `KickoffMarkStarting`(member_id: AgentIdentity)
- `StartupMarkReady`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `KickoffResolveStarted`(member_id: AgentIdentity)
- `KickoffResolveCallbackPending`(member_id: AgentIdentity)
- `KickoffResolveFailed`(member_id: AgentIdentity, error: String)
- `KickoffCancelRequested`(member_id: AgentIdentity)
- `KickoffClear`(member_id: AgentIdentity)
- `KickoffQuiesced`(member_id: AgentIdentity)
- `CancelPendingSpawn`(agent_identity: AgentIdentity)
- `ProbeMemberAdmission`(agent_identity: AgentIdentity)
- `ComputeRespawnGeneration`(agent_identity: AgentIdentity)
- `ClassifyStepOutputFault`(run_id: RunId, step_id: StepId, target_retry_key: String, fault: StepOutputFaultKind, attempt: u32, max_retries: u32)
- `EscalateToSupervisor`(run_id: RunId, step_id: StepId, supervisor_identity: AgentIdentity, turn_timeout_ms: u64)
- `EscalateToSupervisorNoEligibleTarget`(run_id: RunId, step_id: StepId)
- `EvaluateTopologyEdge`(from_role: String, to_role: String, rule_match: Option<PolicyDecision>)
- `SetExternalMemberRebindCapability`(agent_identity: AgentIdentity, capability: ExternalMemberRebindCapability)
- `ClassifyTurnTimeoutDisposition`(timed_out_run_id: RunId, retryable: Bool)
- `SeedOrphanBudget`(budget: u64)
- `RecordCoordinationWorkIntent`(intent_id: WorkIntentId, requested_status: MobCoordinationWorkIntentStatus, owner_present: Bool, summary_present: Bool, metadata_public: Bool, draft_mob_id: MobId, authority_mob_id: MobId, resource_tokens: Set<CoordinationResourceRef>, expires_at_ms: Option<u64>)
- `RecordCoordinationResourceClaim`(claim_id: ResourceClaimId, requested_kind: MobCoordinationResourceClaimKind, requested_status: MobCoordinationResourceClaimStatus, owner_present: Bool, metadata_public: Bool, draft_mob_id: MobId, authority_mob_id: MobId, resource_tokens: Set<CoordinationResourceRef>, expires_at_ms: Option<u64>)
- `UpdateCoordinationWorkIntentStatus`(intent_id: WorkIntentId, expected_revision: u64, requested_status: MobCoordinationWorkIntentStatus, now_ms: u64)
- `UpdateCoordinationResourceClaimStatus`(claim_id: ResourceClaimId, expected_revision: u64, requested_status: MobCoordinationResourceClaimStatus, now_ms: u64)
- `ObserveCoordinationResourceClaimOverlap`(claim_id: ResourceClaimId, now_ms: u64, candidate_overlap_ids: Seq<ResourceClaimId>)
- `InitializeAdaptiveRun`(adaptive_run_id: AdaptiveRunId, max_depth: u64, max_total_decisions: u64, max_repair_attempts: u64, max_layer_failures: u64, max_attempts_per_layer: u64, max_members_per_layer: u64, max_total_spawned_members: u64, max_active_members: u64, max_retained_layer_mobs: u64, max_aggregate_tokens: u64, max_aggregate_tool_calls: u64, allowed_model_classes: Set<String>, allowed_tool_classes: Set<String>, allowed_skill_identities: Set<String>, allowed_auth_binding_refs: Set<String>, deadline_ms: u64)
- `RecordPlanningDecision`(adaptive_run_id: AdaptiveRunId, decision_kind: AdaptiveDecisionKind)
- `RecordPlanRejected`(adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId)
- `ResolveLayerAdmission`(adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, attempt: u64, plan_digest: String, child_mob_id: MobId, member_count: u64, token_reservation: u64, tool_call_reservation: u64, used_model_classes: Set<String>, used_tool_classes: Set<String>, used_skill_identities: Set<String>, used_auth_binding_refs: Set<String>, observed_at_ms: u64)
- `RecordLayerProvisioned`(adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, attempt: u64)
- `RecordLayerRunStarted`(adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, attempt: u64, child_run_id: RunId)
- `IngestLayerTerminal`(adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, attempt: u64, result_class: FlowRunPublicResultClassKind, actual_tokens: u64, actual_tool_calls: u64)
- `RecordLayerSetupFault`(adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, attempt: u64, fault: AdaptiveLayerSetupFaultKind, spawned_members: u64, requested_members: u64)
- `RecordLayerResultValidated`(adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, attempt: u64, result_digest: String)
- `RecordLayerResultInvalid`(adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, attempt: u64)
- `RecordLayerMobDestroyed`(adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, attempt: u64)
- `RecordLayerMobRetained`(adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, attempt: u64, disposition: AdaptiveLayerDispositionKind)
- `RecordCleanupResolved`(adaptive_run_id: AdaptiveRunId)
- `RecordBodyEvidenceMissing`(adaptive_run_id: AdaptiveRunId, missing_digest: String)
- `ResolveAdaptiveFinish`(adaptive_run_id: AdaptiveRunId, final_result_digest: String)
- `RequestAdaptiveCancel`(adaptive_run_id: AdaptiveRunId)
- `RecordDeadlineObserved`(adaptive_run_id: AdaptiveRunId, observed_at_ms: u64)

## Signals
- `ObserveRuntimeReady`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `AdmitDestroyMemberRetire`(mob_id: MobId, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, session_id: Option<SessionId>)
- `ObserveRuntimeRetired`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `ObserveMemberRetirementArchived`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, session_id: Option<SessionId>)
- `ObserveRemoteMemberRetirementArchivedAndSupervisorRevoked`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `ObserveDestroyMemberRetirementArchived`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, session_id: Option<SessionId>)
- `ResetMember`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, profile_name: String, runtime_mode: SpawnPolicyRuntimeMode, external_addressable: Bool, session_id: SessionId)
- `RespawnMember`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, profile_name: String, runtime_mode: SpawnPolicyRuntimeMode, external_addressable: Bool, session_id: SessionId)
- `ResolveRespawnTopologyRestore`(agent_identity: AgentIdentity, failed_peer_ids: Seq<RespawnTopologyPeerId>)
- `DestroyMob`(session_id: SessionId)
- `ObserveRuntimeDestroyed`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `RecoverRosterMember`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, profile_name: String, runtime_mode: SpawnPolicyRuntimeMode, external_addressable: Bool)
- `RecoverMemberSessionBinding`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, bridge_session_id: SessionId, replacing: Option<SessionId>)
- `RecoverSpawnedMemberPeerEndpoint`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, peer_endpoint: MemberPeerEndpoint)
- `RecoverMemberPeerEndpoint`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, bridge_session_id: SessionId, peer_endpoint: MemberPeerEndpoint)
- `RecoverRosterMemberReset`(agent_identity: AgentIdentity, previous_agent_runtime_id: AgentRuntimeId, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `RecoverRosterMemberRetirementStarted`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, generation: Generation, releasing: Option<SessionId>, session_id: Option<SessionId>, retiring_peer_endpoint: Option<MemberPeerEndpoint>)
- `RecoverRemoteMemberRuntimeRetired`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `RecoverRemoteMemberSupervisorRevoked`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `RecoverRosterMemberRetired`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId)
- `ConvergeRecoveredRosterTopology`(edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity)
- `RecoverMemberKickoff`(member_id: AgentIdentity, phase: KickoffPhase, error: Option<String>)
- `RecoverRosterWiring`(edge: WiringEdge)
- `RecoverRosterUnwire`(edge: WiringEdge)
- `RecoverExternalPeerWiring`(key: ExternalPeerKey, edge: ExternalPeerEdge)
- `RecoverExternalPeerUnwire`(key: ExternalPeerKey)
- `RecoverSupervisorAuthority`(peer_id: PeerId, signing_key: PeerSigningKey, epoch: u64, protocol_version: SupervisorProtocolVersion, pending_operation_id: Option<String>, pending_peer_id: Option<PeerId>, pending_signing_key: Option<PeerSigningKey>, pending_epoch: Option<u64>, pending_protocol_version: Option<SupervisorProtocolVersion>, pending_accepted_peer_ids: Set<PeerId>, pending_member_target_names: Map<PeerId, String>, pending_member_target_addresses: Map<PeerId, String>)
- `RecoverOwnerBridgeSession`(bridge_session_id: SessionId, destroy_on_owner_archive: Bool, implicit_delegation_mob: Bool)
- `RecoverMemberRestoreFailure`(agent_identity: AgentIdentity, reason: String)
- `ClassifyMemberLiveMaterialization`(agent_identity: AgentIdentity, observation: MemberLiveMaterializationObservationKind, reason: String)
- `ResolveMemberRevivalSucceeded`(agent_identity: AgentIdentity)
- `ResolveMemberRevivalFailed`(agent_identity: AgentIdentity, reason: String)
- `AdmitDestroyCleanup`
- `AdmitDestroyStorageFinalizing`
- `MarkCompleted`
- `StartRun`
- `FinishRun`
- `BeginCleanup`
- `FinishCleanup`
- `InitializeOrchestrator`
- `BindCoordinator`
- `UnbindCoordinator`
- `StageSpawn`(agent_identity: AgentIdentity, session_id: SessionId)
- `CompleteSpawn`(agent_identity: AgentIdentity)
- `StartFlow`
- `CompleteFlow`
- `StopOrchestrator`
- `ResumeOrchestrator`
- `DestroyOrchestrator`
- `ForceCancelMember`
- `MemberPeerExposed`
- `MemberTerminalized`
- `OperationPeerTrusted`
- `PeerInputAdmitted`
- `CreateRun`

## Effects
- `RequestRuntimeBinding`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Option<Generation>, session_id: SessionId)
- `SpawnProfileAuthorized`(agent_identity: AgentIdentity, profile_name: String, model: String, profile_material_digest: String, tool_config_digest: String, skills_digest: String, provider_params_digest: Option<String>, output_schema_digest: Option<String>, external_addressable: Bool)
- `RequestRuntimeIngress`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Option<Generation>, session_id: SessionId, work_id: WorkId, origin: WorkOrigin)
- `RequestPeerRuntimeIngress`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Option<Generation>, work_id: WorkId, origin: WorkOrigin)
- `SubmitWorkRejected`(agent_runtime_id: AgentRuntimeId, origin: WorkOrigin, reason: SubmitWorkRejectReasonKind, expected_fence_token: Option<FenceToken>, actual_fence_token: Option<FenceToken>)
- `CancelAllWorkRejected`(agent_runtime_id: AgentRuntimeId, reason: CancelAllWorkRejectReasonKind, expected_fence_token: Option<FenceToken>, actual_fence_token: Option<FenceToken>)
- `RequestRuntimeRetire`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, session_id: SessionId)
- `RequestRuntimeDestroy`(session_id: SessionId)
- `RuntimeBindingRefusalClassified`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, session_id: SessionId, refusal_code: String, reason: String)
- `RuntimeIngressRefusalClassified`(agent_runtime_id: AgentRuntimeId, session_id: SessionId, work_id: WorkId, origin: WorkOrigin, refusal_code: String, reason: String)
- `RuntimeRetireRefusalClassified`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, session_id: SessionId, refusal_code: String, reason: String)
- `PendingSpawnOperationOwnerAuthorized`(agent_identity: AgentIdentity, session_id: SessionId)
- `RequestSessionIngressDetachForMobDestroy`(mob_id: MobId, agent_runtime_id: AgentRuntimeId)
- `AppendLifecycleJournal`(kind: MobLifecycleJournalKind, agent_identity: Option<AgentIdentity>, agent_runtime_id: Option<AgentRuntimeId>, fence_token: Option<FenceToken>, generation: Option<Generation>, session_id: Option<SessionId>)
- `AppendOperatorActionProvenance`(tool_name: String, principal_token: OpaquePrincipalToken, caller_provenance: Option<MobToolCallerProvenance>, audit_invocation_id: Option<String>)
- `EmitMemberLifecycleNotice`(kind: MemberLifecycleKind)
- `EmitRunLifecycleNotice`
- `EmitFlowRunNotice`
- `AppendFailureLedger`
- `FlowTerminalized`
- `FlowRunTerminal`(run_id: RunId)
- `FlowRunNonTerminal`(run_id: RunId)
- `FlowStepTerminal`(run_id: RunId, step_id: StepId)
- `FlowStepNonTerminal`(run_id: RunId, step_id: StepId)
- `FlowFrameTerminalStatusClassified`(frame_id: FrameId, terminal_status: FrameStatus)
- `FlowFrameTerminalStatusUnavailable`(frame_id: FrameId)
- `FlowRunPublicResultClassified`(run_id: RunId, result: FlowRunPublicResultClassKind)
- `EscalateSupervisor`
- `NotifyCoordinator`
- `ExposePendingSpawn`
- `EmitMemberTerminalNotice`
- `AdmitPeerInput`
- `EmitProgressNote`
- `PersistKickoffUpdate`(member_id: AgentIdentity, phase: KickoffPhase)
- `PersistKickoffFailureUpdate`(member_id: AgentIdentity, phase: KickoffPhase, error: String)
- `EmitKickoffLifecycleNotice`(member_id: AgentIdentity, intent: KickoffIntent)
- `RequestKickoffQuiesce`(member_id: AgentIdentity)
- `RequestPendingSpawnQuiesceForDestroy`
- `MemberAdmissionProbed`(agent_identity: AgentIdentity, verdict: MemberAdmissionVerdictKind)
- `RespawnGenerationComputed`(agent_identity: AgentIdentity, next_generation: Generation)
- `StepOutputFaultClassified`(run_id: RunId, step_id: StepId, target_retry_key: String, disposition: StepFaultDispositionKind, terminal_cause: StepOutputFaultKind)
- `SupervisorEscalationRequested`(run_id: RunId, step_id: StepId, supervisor_identity: AgentIdentity, turn_timeout_ms: u64)
- `SupervisorEscalationFailed`(run_id: RunId, step_id: StepId, cause: SupervisorEscalationFailureCause)
- `TopologyEdgeVerdictResolved`(from_role: String, to_role: String, verdict: PolicyDecision)
- `TurnTimeoutDispositionClassified`(timed_out_run_id: RunId, disposition: TurnTimeoutDisposition)
- `MemberSpawnRequired`(agent_identity: AgentIdentity)
- `MemberRetainRequired`(agent_identity: AgentIdentity)
- `MemberRetireRequired`(agent_identity: AgentIdentity)
- `SpawnPolicyResolutionRecorded`(agent_identity: AgentIdentity, revision: u64, profile_name: Option<String>, runtime_mode: Option<SpawnPolicyRuntimeMode>)
- `OwnerBridgeSessionBound`(bridge_session_id: SessionId, destroy_on_owner_archive: Bool, implicit_delegation_mob: Bool)
- `RespawnTopologyRestoreResolved`(agent_identity: AgentIdentity, result: RespawnTopologyRestoreResultKind, failed_peer_ids: Seq<RespawnTopologyPeerId>)
- `MemberLiveMaterializationClassified`(agent_identity: AgentIdentity, observation: MemberLiveMaterializationObservationKind, verdict: MemberRevivalVerdictKind, reason: String)
- `SpawnManyFailureClassified`(observation: MobSpawnManyFailureObservationKind, cause: MobSpawnManyFailureCauseKind)
- `MemberWaitClassified`(agent_identity: AgentIdentity, result: MemberWaitClassificationKind)
- `FlowDelegationEdgeAdmissionResolved`(from_role: String, to_role: String, admission: MobFlowDelegationEdgeAdmissionKind)
- `RemoteMemberRuntimeTerminalityClassified`(observed_state: MobRemoteMemberRuntimeObservedState, terminality: MobRemoteMemberRuntimeTerminality)
- `SpawnMemberAdmissionResolved`(admission: MobSpawnMemberAdmissionKind)
- `CurrentMobAdmissionResolved`(admission: MobCurrentMobAdmissionKind)
- `SpawnToolAdmissionResolved`(admission: MobSpawnToolAdmissionKind)
- `CreateMobAdmissionResolved`(admission: MobCreateMobAdmissionKind)
- `ProfileMutationAdmissionResolved`(admission: MobProfileMutationAdmissionKind)
- `MemberOperationEligibilityResolved`(admission: MobMemberOperationEligibilityKind)
- `BridgeRejectionRecoveryClassified`(rejection_cause: MobBridgeRejectionCause, recovery: MobBridgeRejectionRecovery)
- `PendingSupervisorAcceptanceClassified`(rejection_cause: MobBridgeRejectionCause, verdict: MobPendingSupervisorAcceptanceKind)
- `FrameSeedConfirmed`(frame_id: FrameId, disposition: MobFrameSeedDisposition)
- `WiringGraphChanged`(epoch: u64)
- `MemberSessionBindingChanged`(epoch: u64, agent_identity: AgentIdentity, old_session_id: Option<SessionId>, new_session_id: Option<SessionId>)
- `SessionProvisionOperationOwnerAuthorized`(agent_identity: AgentIdentity, session_id: SessionId)
- `MemberTrustWiringRequested`(edge: WiringEdge, a_peer_id: PeerId, b_peer_id: PeerId, a_endpoint: MemberPeerEndpoint, b_endpoint: MemberPeerEndpoint, epoch: u64)
- `MemberTrustUnwiringRequested`(edge: WiringEdge, a_peer_id: PeerId, b_peer_id: PeerId, epoch: u64)
- `WiringTrustRepairRequested`(edge: WiringEdge)
- `ExternalPeerTrustWiringRequested`(edge: ExternalPeerEdge, local_peer_id: PeerId, peer_id: PeerId, epoch: u64)
- `ExternalPeerTrustUnwiringRequested`(edge: ExternalPeerEdge, local_peer_id: PeerId, peer_id: PeerId, epoch: u64)
- `ExternalPeerTrustRepairRequested`(edge: ExternalPeerEdge, local_peer_id: PeerId, peer_id: PeerId, epoch: u64)
- `MemberPeerRegistered`(agent_identity: AgentIdentity, peer_id: PeerId)
- `MemberPeerRebindAuthorized`(agent_identity: AgentIdentity, peer_id: PeerId, peer_endpoint: MemberPeerEndpoint)
- `MemberPeerOverlayAuthorized`(agent_identity: AgentIdentity, peer_id: PeerId, peer_overlay_endpoints: Set<MemberPeerEndpoint>, epoch: u64)
- `ExternalPeerReciprocalTrustRequested`(key: ExternalPeerKey, edge: ExternalPeerEdge, peer_id: PeerId, peer_endpoint: MemberPeerEndpoint, epoch: u64)
- `PersistSupervisorAuthority`(peer_id: PeerId, signing_key: PeerSigningKey, epoch: u64, protocol_version: SupervisorProtocolVersion, pending_operation_id: Option<String>, pending_peer_id: Option<PeerId>, pending_signing_key: Option<PeerSigningKey>, pending_epoch: Option<u64>, pending_protocol_version: Option<SupervisorProtocolVersion>, pending_accepted_peer_ids: Set<PeerId>, pending_member_target_names: Map<PeerId, String>, pending_member_target_addresses: Map<PeerId, String>)
- `DeleteSupervisorAuthority`(peer_id: PeerId, signing_key: PeerSigningKey, epoch: u64, protocol_version: SupervisorProtocolVersion)
- `EmitWiringLifecycleNotice`(kind: WiringLifecycleKind, edge: WiringEdge)
- `EmitExternalPeerWiringLifecycleNotice`(kind: WiringLifecycleKind, edge: ExternalPeerEdge)
- `AuthorizeAgentEventSubscription`(agent_identity: AgentIdentity, session_id: SessionId)
- `RejectAgentEventSubscription`(agent_identity: AgentIdentity, reason: EventSubscriptionRejectReasonKind)
- `AuthorizeAllAgentEventSubscription`(session_bound_runtimes: Set<AgentRuntimeId>)
- `RejectAllAgentEventSubscription`(reason: EventSubscriptionRejectReasonKind)
- `AuthorizeMobEventRouter`(initial_cursor: u64, channel_capacity: u64, poll_interval_ms: u64, session_bound_runtimes: Set<AgentRuntimeId>)
- `AuthorizeMobEventRouterMemberSubscription`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, session_id: SessionId)
- `AuthorizeMobEventRouterMemberRemoval`(agent_identity: AgentIdentity)
- `AuthorizeStructuralEventSubscription`(after_cursor: u64, explicit_after_cursor: Bool, batch_limit: u64, channel_capacity: u64)
- `RejectStructuralEventSubscription`(after_cursor: u64, latest_cursor: u64)
- `AuthorizeStrictEventPoll`(after_cursor: u64, limit: u64)
- `RejectStrictEventPoll`(after_cursor: u64, latest_cursor: u64)
- `WorkIntentRecorded`(intent_id: WorkIntentId, status: MobCoordinationWorkIntentStatus, revision: u64, resource_tokens: Set<CoordinationResourceRef>, expires_at_ms: Option<u64>, event_kind: MobCoordinationEventKind, sequence: u64)
- `ResourceClaimRecorded`(claim_id: ResourceClaimId, kind: MobCoordinationResourceClaimKind, status: MobCoordinationResourceClaimStatus, revision: u64, resource_tokens: Set<CoordinationResourceRef>, expires_at_ms: Option<u64>, event_kind: MobCoordinationEventKind, sequence: u64)
- `WorkIntentStatusChanged`(intent_id: WorkIntentId, status: MobCoordinationWorkIntentStatus, revision: u64, event_kind: MobCoordinationEventKind, sequence: u64)
- `ResourceClaimStatusChanged`(claim_id: ResourceClaimId, status: MobCoordinationResourceClaimStatus, revision: u64, event_kind: MobCoordinationEventKind, sequence: u64)
- `ResourceClaimOverlapObserved`(claim_id: ResourceClaimId, overlap_ids: Seq<ResourceClaimId>, event_kind: MobCoordinationEventKind, sequence: u64)
- `AdaptiveRunInitialized`(adaptive_run_id: AdaptiveRunId)
- `AdaptivePlanningDecisionRecorded`(adaptive_run_id: AdaptiveRunId, decision_kind: AdaptiveDecisionKind)
- `AdaptivePlanRejected`(adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId)
- `AdaptiveLayerAdmissionResolved`(adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, admission: AdaptiveLayerAdmissionKind)
- `AdaptiveLayerProvisioned`(adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, attempt: u64)
- `AdaptiveLayerRunStarted`(adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, attempt: u64, child_run_id: RunId)
- `AdaptiveLayerTerminalIngested`(adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, result_class: FlowRunPublicResultClassKind)
- `AdaptiveLayerSetupFaultRecorded`(adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, fault: AdaptiveLayerSetupFaultKind)
- `AdaptiveLayerResultValidated`(adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, result_digest: String)
- `AdaptiveLayerResultInvalid`(adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId)
- `AdaptiveLayerCleanupObserved`(adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, disposition: AdaptiveLayerDispositionKind)
- `AdaptiveCleanupResolved`(adaptive_run_id: AdaptiveRunId)
- `AdaptiveBodyEvidenceMissing`(adaptive_run_id: AdaptiveRunId, missing_digest: String)
- `AdaptiveRunTerminalized`(adaptive_run_id: AdaptiveRunId, reason: AdaptiveStopReason)

## Command Plans
### `AuthorizedMobSpawnStart`
- Authority: `PendingSpawnOperationOwnerAuthorized`
- Source Inputs: `CancelPendingSpawn`
- Source Signals: `StageSpawn`, `CompleteSpawn`
- Transitions: `StageSpawnRunning`, `CompleteSpawnRunning`, `CompleteSpawnLateArrivalRunning`, `CompleteSpawnLateArrivalStopped`, `CompleteSpawnLateArrivalCompleted`, `CompleteSpawnDestroyed`, `CancelPendingSpawnPresentRunning`, `CancelPendingSpawnPresentStopped`, `CancelPendingSpawnPresentCompleted`, `CancelPendingSpawnAbsentRunning`, `CancelPendingSpawnAbsentStopped`, `CancelPendingSpawnAbsentCompleted`, `CancelPendingSpawnDestroyed`
- Guard Expansion:
  - `StageSpawnRunning`: `pending_identity_unused`
  - `CompleteSpawnRunning`: `pending_spawns_present`, `pending_identity_present`
  - `CompleteSpawnLateArrivalRunning`: `pending_identity_absent`
  - `CompleteSpawnLateArrivalStopped`: `pending_identity_absent`
  - `CompleteSpawnLateArrivalCompleted`: `pending_identity_absent`
  - `CompleteSpawnDestroyed`: `<none>`
  - `CancelPendingSpawnPresentRunning`: `pending_spawns_present`, `pending_identity_present`
  - `CancelPendingSpawnPresentStopped`: `pending_spawns_present`, `pending_identity_present`
  - `CancelPendingSpawnPresentCompleted`: `pending_spawns_present`, `pending_identity_present`
  - `CancelPendingSpawnAbsentRunning`: `pending_identity_absent`
  - `CancelPendingSpawnAbsentStopped`: `pending_identity_absent`
  - `CancelPendingSpawnAbsentCompleted`: `pending_identity_absent`
  - `CancelPendingSpawnDestroyed`: `<none>`
- Command Effects: `PendingSpawnOperationOwnerAuthorized`, `ExposePendingSpawn`, `EmitMemberLifecycleNotice`
- Effect Closure:
  - `PendingSpawnOperationOwnerAuthorized` via `PendingSpawnOperationOwnerAuthorized` (LocalPendingSpawnOwner) states: `Authorized`, `Attempted`, `Realized`, `Failed`, `Cancelled`, `Abandoned`
  - `EmitMemberLifecycleNotice` via `CompleteSpawn` (LocalSpawnCompletion) states: `Authorized`, `Attempted`, `Realized`, `Failed`, `Cancelled`, `Abandoned`
- Emitted By Transitions: `EmitMemberLifecycleNotice`, `ExposePendingSpawn`, `PendingSpawnOperationOwnerAuthorized`

### `CanStartSpawn`
- Authority: `CanStartSpawn`
- Source Signals: `StageSpawn`
- Transitions: `StageSpawnRunning`
- Guard Expansion:
  - `StageSpawnRunning`: `pending_identity_unused`
- Command Effects: `PendingSpawnOperationOwnerAuthorized`
- Effect Closure:
  - `PendingSpawnOperationOwnerAuthorized` via `CanStartSpawn` (LocalPendingSpawnOwner) states: `Authorized`, `Attempted`, `Realized`, `Failed`, `Cancelled`, `Abandoned`
- Emitted By Transitions: `ExposePendingSpawn`, `PendingSpawnOperationOwnerAuthorized`

### `SpawnStarted`
- Authority: `SpawnStarted`
- Source Signals: `StageSpawn`
- Transitions: `StageSpawnRunning`
- Guard Expansion:
  - `StageSpawnRunning`: `pending_identity_unused`
- Command Effects: `ExposePendingSpawn`
- Effect Closure:
  - `ExposePendingSpawn` via `SpawnStarted` (LocalSpawnStarted) states: `Authorized`, `Attempted`, `Realized`, `Failed`, `Cancelled`, `Abandoned`
- Emitted By Transitions: `ExposePendingSpawn`, `PendingSpawnOperationOwnerAuthorized`

### `SpawnEffect`
- Authority: `SpawnEffect`
- Source Inputs: `CancelPendingSpawn`
- Source Signals: `CompleteSpawn`
- Transitions: `StageSpawnRunning`, `CompleteSpawnRunning`, `CompleteSpawnLateArrivalRunning`, `CompleteSpawnLateArrivalStopped`, `CompleteSpawnLateArrivalCompleted`, `CompleteSpawnDestroyed`, `CancelPendingSpawnPresentRunning`, `CancelPendingSpawnPresentStopped`, `CancelPendingSpawnPresentCompleted`, `CancelPendingSpawnAbsentRunning`, `CancelPendingSpawnAbsentStopped`, `CancelPendingSpawnAbsentCompleted`, `CancelPendingSpawnDestroyed`
- Guard Expansion:
  - `StageSpawnRunning`: `pending_identity_unused`
  - `CompleteSpawnRunning`: `pending_spawns_present`, `pending_identity_present`
  - `CompleteSpawnLateArrivalRunning`: `pending_identity_absent`
  - `CompleteSpawnLateArrivalStopped`: `pending_identity_absent`
  - `CompleteSpawnLateArrivalCompleted`: `pending_identity_absent`
  - `CompleteSpawnDestroyed`: `<none>`
  - `CancelPendingSpawnPresentRunning`: `pending_spawns_present`, `pending_identity_present`
  - `CancelPendingSpawnPresentStopped`: `pending_spawns_present`, `pending_identity_present`
  - `CancelPendingSpawnPresentCompleted`: `pending_spawns_present`, `pending_identity_present`
  - `CancelPendingSpawnAbsentRunning`: `pending_identity_absent`
  - `CancelPendingSpawnAbsentStopped`: `pending_identity_absent`
  - `CancelPendingSpawnAbsentCompleted`: `pending_identity_absent`
  - `CancelPendingSpawnDestroyed`: `<none>`
- Command Effects: `EmitMemberLifecycleNotice`
- Effect Closure:
  - `EmitMemberLifecycleNotice` via `SpawnEffect` (LocalSpawnCompletion) states: `Authorized`, `Attempted`, `Realized`, `Failed`, `Cancelled`, `Abandoned`
- Emitted By Transitions: `EmitMemberLifecycleNotice`, `ExposePendingSpawn`, `PendingSpawnOperationOwnerAuthorized`

### `FailSpawn`
- Authority: `FailSpawn`
- Source Inputs: `CancelPendingSpawn`
- Transitions: `CancelPendingSpawnPresentRunning`, `CancelPendingSpawnPresentStopped`, `CancelPendingSpawnPresentCompleted`, `CancelPendingSpawnAbsentRunning`, `CancelPendingSpawnAbsentStopped`, `CancelPendingSpawnAbsentCompleted`, `CancelPendingSpawnDestroyed`
- Guard Expansion:
  - `CancelPendingSpawnPresentRunning`: `pending_spawns_present`, `pending_identity_present`
  - `CancelPendingSpawnPresentStopped`: `pending_spawns_present`, `pending_identity_present`
  - `CancelPendingSpawnPresentCompleted`: `pending_spawns_present`, `pending_identity_present`
  - `CancelPendingSpawnAbsentRunning`: `pending_identity_absent`
  - `CancelPendingSpawnAbsentStopped`: `pending_identity_absent`
  - `CancelPendingSpawnAbsentCompleted`: `pending_identity_absent`
  - `CancelPendingSpawnDestroyed`: `<none>`
- Command Effects: `EmitMemberLifecycleNotice`
- Effect Closure:
  - `EmitMemberLifecycleNotice` via `FailSpawn` (LocalSpawnFailure) states: `Authorized`, `Attempted`, `Realized`, `Failed`, `Cancelled`, `Abandoned`

## Invariants
- `bindings_require_known_identity`
- `identity_runtime_material_matches_runtime_binding`
- `member_spawn_material_matches_runtime_binding`
- `member_peer_endpoint_material_is_coherent`
- `member_prior_peer_endpoints_are_generation_scoped`
- `member_peer_id_ownership_is_global_across_generations`
- `pending_session_ingress_detach_has_session_correlation`
- `remote_runtime_retired_is_still_retiring`
- `remote_supervisor_revoked_has_retired_runtime_anchor`
- `spawn_exec_membership_commit_requires_runtime`
- `external_peer_edges_are_keyed_coherently`
- `supervisor_authority_tuple_consistent`
- `supervisor_pending_authority_tuple_consistent`
- `supervisor_pending_authority_member_targets_aligned`
- `supervisor_pending_authority_acceptance_has_target`
- `supervisor_pending_authority_requires_current`
- `owner_bridge_cleanup_requires_owner`
- `implicit_delegation_requires_owner`
- `implicit_delegation_requires_cleanup`

## Transitions
### `ClassifyFlowRunTerminalityTerminalRunning`
- From: `Running`
- On: `ClassifyFlowRunTerminality`(run_id, status)
- Guards:
  - `run_status_terminal`
- Emits: `FlowRunTerminal`
- To: `Running`

### `ClassifyFlowRunTerminalityNonTerminalRunning`
- From: `Running`
- On: `ClassifyFlowRunTerminality`(run_id, status)
- Guards:
  - `run_status_non_terminal`
- Emits: `FlowRunNonTerminal`
- To: `Running`

### `ClassifyFlowStepTerminalityTerminalRunning`
- From: `Running`
- On: `ClassifyFlowStepTerminality`(run_id, step_id, status)
- Guards:
  - `step_status_terminal`
- Emits: `FlowStepTerminal`
- To: `Running`

### `ClassifyFlowStepTerminalityNonTerminalRunning`
- From: `Running`
- On: `ClassifyFlowStepTerminality`(run_id, step_id, status)
- Guards:
  - `step_status_non_terminal`
- Emits: `FlowStepNonTerminal`
- To: `Running`

### `ClassifyFlowFrameTerminalStatusFailedRunning`
- From: `Running`
- On: `ClassifyFlowFrameTerminalStatus`(frame_id)
- Guards:
  - `known_frame`
  - `frame_tracked_nodes_known`
  - `frame_node_status_known`
  - `frame_running`
  - `all_nodes_terminal`
  - `any_node_failed`
- Emits: `FlowFrameTerminalStatusClassified`
- To: `Running`

### `ClassifyFlowFrameTerminalStatusFailedStopped`
- From: `Stopped`
- On: `ClassifyFlowFrameTerminalStatus`(frame_id)
- Guards:
  - `known_frame`
  - `frame_tracked_nodes_known`
  - `frame_node_status_known`
  - `frame_running`
  - `all_nodes_terminal`
  - `any_node_failed`
- Emits: `FlowFrameTerminalStatusClassified`
- To: `Stopped`

### `ClassifyFlowFrameTerminalStatusFailedCompleted`
- From: `Completed`
- On: `ClassifyFlowFrameTerminalStatus`(frame_id)
- Guards:
  - `known_frame`
  - `frame_tracked_nodes_known`
  - `frame_node_status_known`
  - `frame_running`
  - `all_nodes_terminal`
  - `any_node_failed`
- Emits: `FlowFrameTerminalStatusClassified`
- To: `Completed`

### `ClassifyFlowFrameTerminalStatusCanceledRunning`
- From: `Running`
- On: `ClassifyFlowFrameTerminalStatus`(frame_id)
- Guards:
  - `known_frame`
  - `frame_tracked_nodes_known`
  - `frame_node_status_known`
  - `frame_running`
  - `all_nodes_terminal`
  - `no_node_failed`
  - `any_node_canceled`
- Emits: `FlowFrameTerminalStatusClassified`
- To: `Running`

### `ClassifyFlowFrameTerminalStatusCanceledStopped`
- From: `Stopped`
- On: `ClassifyFlowFrameTerminalStatus`(frame_id)
- Guards:
  - `known_frame`
  - `frame_tracked_nodes_known`
  - `frame_node_status_known`
  - `frame_running`
  - `all_nodes_terminal`
  - `no_node_failed`
  - `any_node_canceled`
- Emits: `FlowFrameTerminalStatusClassified`
- To: `Stopped`

### `ClassifyFlowFrameTerminalStatusCanceledCompleted`
- From: `Completed`
- On: `ClassifyFlowFrameTerminalStatus`(frame_id)
- Guards:
  - `known_frame`
  - `frame_tracked_nodes_known`
  - `frame_node_status_known`
  - `frame_running`
  - `all_nodes_terminal`
  - `no_node_failed`
  - `any_node_canceled`
- Emits: `FlowFrameTerminalStatusClassified`
- To: `Completed`

### `ClassifyFlowFrameTerminalStatusCompletedRunning`
- From: `Running`
- On: `ClassifyFlowFrameTerminalStatus`(frame_id)
- Guards:
  - `known_frame`
  - `frame_tracked_nodes_known`
  - `frame_node_status_known`
  - `frame_running`
  - `all_nodes_terminal`
  - `no_node_failed`
  - `no_node_canceled`
- Emits: `FlowFrameTerminalStatusClassified`
- To: `Running`

### `ClassifyFlowFrameTerminalStatusCompletedStopped`
- From: `Stopped`
- On: `ClassifyFlowFrameTerminalStatus`(frame_id)
- Guards:
  - `known_frame`
  - `frame_tracked_nodes_known`
  - `frame_node_status_known`
  - `frame_running`
  - `all_nodes_terminal`
  - `no_node_failed`
  - `no_node_canceled`
- Emits: `FlowFrameTerminalStatusClassified`
- To: `Stopped`

### `ClassifyFlowFrameTerminalStatusCompletedCompleted`
- From: `Completed`
- On: `ClassifyFlowFrameTerminalStatus`(frame_id)
- Guards:
  - `known_frame`
  - `frame_tracked_nodes_known`
  - `frame_node_status_known`
  - `frame_running`
  - `all_nodes_terminal`
  - `no_node_failed`
  - `no_node_canceled`
- Emits: `FlowFrameTerminalStatusClassified`
- To: `Completed`

### `ClassifyFlowFrameTerminalStatusUnavailableRunning`
- From: `Running`
- On: `ClassifyFlowFrameTerminalStatus`(frame_id)
- Guards:
  - `known_frame`
  - `frame_tracked_nodes_known`
  - `frame_node_status_known`
  - `terminal_status_unavailable`
- Emits: `FlowFrameTerminalStatusUnavailable`
- To: `Running`

### `ClassifyFlowFrameTerminalStatusUnavailableStopped`
- From: `Stopped`
- On: `ClassifyFlowFrameTerminalStatus`(frame_id)
- Guards:
  - `known_frame`
  - `frame_tracked_nodes_known`
  - `frame_node_status_known`
  - `terminal_status_unavailable`
- Emits: `FlowFrameTerminalStatusUnavailable`
- To: `Stopped`

### `ClassifyFlowFrameTerminalStatusUnavailableCompleted`
- From: `Completed`
- On: `ClassifyFlowFrameTerminalStatus`(frame_id)
- Guards:
  - `known_frame`
  - `frame_tracked_nodes_known`
  - `frame_node_status_known`
  - `terminal_status_unavailable`
- Emits: `FlowFrameTerminalStatusUnavailable`
- To: `Completed`

### `ClassifyFlowRunPublicResultSuccessRunning`
- From: `Running`
- On: `ClassifyFlowRunPublicResult`(run_id, status)
- Guards:
  - `run_status_success`
- Emits: `FlowRunPublicResultClassified`
- To: `Running`

### `ClassifyFlowRunPublicResultFailureRunning`
- From: `Running`
- On: `ClassifyFlowRunPublicResult`(run_id, status)
- Guards:
  - `run_status_failure`
- Emits: `FlowRunPublicResultClassified`
- To: `Running`

### `ClassifyMemberWaitRuntimeMaterialPresentRunning`
- From: `Running`
- On: `ClassifyMemberWait`(agent_identity)
- Guards:
  - `runtime_material_present`
- Emits: `MemberWaitClassified`
- To: `Running`

### `ClassifyMemberWaitRuntimeMaterialPresentStopped`
- From: `Stopped`
- On: `ClassifyMemberWait`(agent_identity)
- Guards:
  - `runtime_material_present`
- Emits: `MemberWaitClassified`
- To: `Stopped`

### `ClassifyMemberWaitRuntimeMaterialPresentCompleted`
- From: `Completed`
- On: `ClassifyMemberWait`(agent_identity)
- Guards:
  - `runtime_material_present`
- Emits: `MemberWaitClassified`
- To: `Completed`

### `ClassifyMemberWaitRuntimeMaterialPresentDestroyed`
- From: `Destroyed`
- On: `ClassifyMemberWait`(agent_identity)
- Guards:
  - `runtime_material_present`
- Emits: `MemberWaitClassified`
- To: `Destroyed`

### `ClassifyMemberWaitMissingRuntimeMaterialRunning`
- From: `Running`
- On: `ClassifyMemberWait`(agent_identity)
- Guards:
  - `runtime_material_missing`
- Emits: `MemberWaitClassified`
- To: `Running`

### `ClassifyMemberWaitMissingRuntimeMaterialStopped`
- From: `Stopped`
- On: `ClassifyMemberWait`(agent_identity)
- Guards:
  - `runtime_material_missing`
- Emits: `MemberWaitClassified`
- To: `Stopped`

### `ClassifyMemberWaitMissingRuntimeMaterialCompleted`
- From: `Completed`
- On: `ClassifyMemberWait`(agent_identity)
- Guards:
  - `runtime_material_missing`
- Emits: `MemberWaitClassified`
- To: `Completed`

### `ClassifyMemberWaitMissingRuntimeMaterialDestroyed`
- From: `Destroyed`
- On: `ClassifyMemberWait`(agent_identity)
- Guards:
  - `runtime_material_missing`
- Emits: `MemberWaitClassified`
- To: `Destroyed`

### `ResolveFlowDelegationEdgeAdmissionAllowedRunning`
- From: `Running`
- On: `ResolveFlowDelegationEdgeAdmission`(from_role, to_role, rule_verdict, mode)
- Guards:
  - `rule_allows_edge`
- Emits: `FlowDelegationEdgeAdmissionResolved`
- To: `Running`

### `ResolveFlowDelegationEdgeAdmissionAllowedStopped`
- From: `Stopped`
- On: `ResolveFlowDelegationEdgeAdmission`(from_role, to_role, rule_verdict, mode)
- Guards:
  - `rule_allows_edge`
- Emits: `FlowDelegationEdgeAdmissionResolved`
- To: `Stopped`

### `ResolveFlowDelegationEdgeAdmissionAllowedCompleted`
- From: `Completed`
- On: `ResolveFlowDelegationEdgeAdmission`(from_role, to_role, rule_verdict, mode)
- Guards:
  - `rule_allows_edge`
- Emits: `FlowDelegationEdgeAdmissionResolved`
- To: `Completed`

### `ResolveFlowDelegationEdgeAdmissionAllowedDestroyed`
- From: `Destroyed`
- On: `ResolveFlowDelegationEdgeAdmission`(from_role, to_role, rule_verdict, mode)
- Guards:
  - `rule_allows_edge`
- Emits: `FlowDelegationEdgeAdmissionResolved`
- To: `Destroyed`

### `ResolveFlowDelegationEdgeAdmissionDeniedStrictRunning`
- From: `Running`
- On: `ResolveFlowDelegationEdgeAdmission`(from_role, to_role, rule_verdict, mode)
- Guards:
  - `rule_denies_edge_strict_mode`
- Emits: `FlowDelegationEdgeAdmissionResolved`
- To: `Running`

### `ResolveFlowDelegationEdgeAdmissionDeniedStrictStopped`
- From: `Stopped`
- On: `ResolveFlowDelegationEdgeAdmission`(from_role, to_role, rule_verdict, mode)
- Guards:
  - `rule_denies_edge_strict_mode`
- Emits: `FlowDelegationEdgeAdmissionResolved`
- To: `Stopped`

### `ResolveFlowDelegationEdgeAdmissionDeniedStrictCompleted`
- From: `Completed`
- On: `ResolveFlowDelegationEdgeAdmission`(from_role, to_role, rule_verdict, mode)
- Guards:
  - `rule_denies_edge_strict_mode`
- Emits: `FlowDelegationEdgeAdmissionResolved`
- To: `Completed`

### `ResolveFlowDelegationEdgeAdmissionDeniedStrictDestroyed`
- From: `Destroyed`
- On: `ResolveFlowDelegationEdgeAdmission`(from_role, to_role, rule_verdict, mode)
- Guards:
  - `rule_denies_edge_strict_mode`
- Emits: `FlowDelegationEdgeAdmissionResolved`
- To: `Destroyed`

### `ResolveFlowDelegationEdgeAdmissionDeniedAdvisoryRunning`
- From: `Running`
- On: `ResolveFlowDelegationEdgeAdmission`(from_role, to_role, rule_verdict, mode)
- Guards:
  - `rule_denies_edge_advisory_mode`
- Emits: `FlowDelegationEdgeAdmissionResolved`
- To: `Running`

### `ResolveFlowDelegationEdgeAdmissionDeniedAdvisoryStopped`
- From: `Stopped`
- On: `ResolveFlowDelegationEdgeAdmission`(from_role, to_role, rule_verdict, mode)
- Guards:
  - `rule_denies_edge_advisory_mode`
- Emits: `FlowDelegationEdgeAdmissionResolved`
- To: `Stopped`

### `ResolveFlowDelegationEdgeAdmissionDeniedAdvisoryCompleted`
- From: `Completed`
- On: `ResolveFlowDelegationEdgeAdmission`(from_role, to_role, rule_verdict, mode)
- Guards:
  - `rule_denies_edge_advisory_mode`
- Emits: `FlowDelegationEdgeAdmissionResolved`
- To: `Completed`

### `ResolveFlowDelegationEdgeAdmissionDeniedAdvisoryDestroyed`
- From: `Destroyed`
- On: `ResolveFlowDelegationEdgeAdmission`(from_role, to_role, rule_verdict, mode)
- Guards:
  - `rule_denies_edge_advisory_mode`
- Emits: `FlowDelegationEdgeAdmissionResolved`
- To: `Destroyed`

### `ClassifyRemoteMemberRuntimeObservationTerminalRunning`
- From: `Running`
- On: `ClassifyRemoteMemberRuntimeObservation`(observed_state)
- Guards:
  - `observed_state_terminal`
- Emits: `RemoteMemberRuntimeTerminalityClassified`
- To: `Running`

### `ClassifyRemoteMemberRuntimeObservationTerminalStopped`
- From: `Stopped`
- On: `ClassifyRemoteMemberRuntimeObservation`(observed_state)
- Guards:
  - `observed_state_terminal`
- Emits: `RemoteMemberRuntimeTerminalityClassified`
- To: `Stopped`

### `ClassifyRemoteMemberRuntimeObservationTerminalCompleted`
- From: `Completed`
- On: `ClassifyRemoteMemberRuntimeObservation`(observed_state)
- Guards:
  - `observed_state_terminal`
- Emits: `RemoteMemberRuntimeTerminalityClassified`
- To: `Completed`

### `ClassifyRemoteMemberRuntimeObservationTerminalDestroyed`
- From: `Destroyed`
- On: `ClassifyRemoteMemberRuntimeObservation`(observed_state)
- Guards:
  - `observed_state_terminal`
- Emits: `RemoteMemberRuntimeTerminalityClassified`
- To: `Destroyed`

### `ClassifyRemoteMemberRuntimeObservationNonTerminalRunning`
- From: `Running`
- On: `ClassifyRemoteMemberRuntimeObservation`(observed_state)
- Guards:
  - `observed_state_non_terminal`
- Emits: `RemoteMemberRuntimeTerminalityClassified`
- To: `Running`

### `ClassifyRemoteMemberRuntimeObservationNonTerminalStopped`
- From: `Stopped`
- On: `ClassifyRemoteMemberRuntimeObservation`(observed_state)
- Guards:
  - `observed_state_non_terminal`
- Emits: `RemoteMemberRuntimeTerminalityClassified`
- To: `Stopped`

### `ClassifyRemoteMemberRuntimeObservationNonTerminalCompleted`
- From: `Completed`
- On: `ClassifyRemoteMemberRuntimeObservation`(observed_state)
- Guards:
  - `observed_state_non_terminal`
- Emits: `RemoteMemberRuntimeTerminalityClassified`
- To: `Completed`

### `ClassifyRemoteMemberRuntimeObservationNonTerminalDestroyed`
- From: `Destroyed`
- On: `ClassifyRemoteMemberRuntimeObservation`(observed_state)
- Guards:
  - `observed_state_non_terminal`
- Emits: `RemoteMemberRuntimeTerminalityClassified`
- To: `Destroyed`

### `ResolveSpawnMemberAdmissionManageScopeRunning`
- From: `Running`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_budget_split_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `manage_scope_allows`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `ResolveSpawnMemberAdmissionManageScopeStopped`
- From: `Stopped`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_budget_split_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `manage_scope_allows`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Stopped`

### `ResolveSpawnMemberAdmissionManageScopeCompleted`
- From: `Completed`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_budget_split_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `manage_scope_allows`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Completed`

### `ResolveSpawnMemberAdmissionManageScopeDestroyed`
- From: `Destroyed`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_budget_split_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `manage_scope_allows`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Destroyed`

### `ResolveSpawnMemberAdmissionPrivilegedArgsDeniedRunning`
- From: `Running`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_budget_split_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `privileged_args_without_manage_scope`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `ResolveSpawnMemberAdmissionPrivilegedArgsDeniedStopped`
- From: `Stopped`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_budget_split_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `privileged_args_without_manage_scope`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Stopped`

### `ResolveSpawnMemberAdmissionPrivilegedArgsDeniedCompleted`
- From: `Completed`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_budget_split_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `privileged_args_without_manage_scope`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Completed`

### `ResolveSpawnMemberAdmissionPrivilegedArgsDeniedDestroyed`
- From: `Destroyed`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_budget_split_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `privileged_args_without_manage_scope`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Destroyed`

### `ResolveSpawnMemberAdmissionProfileScopeRunning`
- From: `Running`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_budget_split_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `profile_scope_allows`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `ResolveSpawnMemberAdmissionProfileScopeStopped`
- From: `Stopped`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_budget_split_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `profile_scope_allows`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Stopped`

### `ResolveSpawnMemberAdmissionProfileScopeCompleted`
- From: `Completed`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_budget_split_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `profile_scope_allows`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Completed`

### `ResolveSpawnMemberAdmissionProfileScopeDestroyed`
- From: `Destroyed`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_budget_split_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `profile_scope_allows`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Destroyed`

### `ResolveSpawnMemberAdmissionDeniedRunning`
- From: `Running`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_budget_split_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `no_scope_denies`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `ResolveSpawnMemberAdmissionDeniedStopped`
- From: `Stopped`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_budget_split_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `no_scope_denies`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Stopped`

### `ResolveSpawnMemberAdmissionDeniedCompleted`
- From: `Completed`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_budget_split_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `no_scope_denies`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Completed`

### `ResolveSpawnMemberAdmissionDeniedDestroyed`
- From: `Destroyed`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_budget_split_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `no_scope_denies`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Destroyed`

### `ResolveCurrentMobAdmissionAllowedRunning`
- From: `Running`
- On: `ResolveCurrentMobAdmission`(can_manage_mob)
- Guards:
  - `manage_scope_allows`
- Emits: `CurrentMobAdmissionResolved`
- To: `Running`

### `ResolveCurrentMobAdmissionAllowedStopped`
- From: `Stopped`
- On: `ResolveCurrentMobAdmission`(can_manage_mob)
- Guards:
  - `manage_scope_allows`
- Emits: `CurrentMobAdmissionResolved`
- To: `Stopped`

### `ResolveCurrentMobAdmissionAllowedCompleted`
- From: `Completed`
- On: `ResolveCurrentMobAdmission`(can_manage_mob)
- Guards:
  - `manage_scope_allows`
- Emits: `CurrentMobAdmissionResolved`
- To: `Completed`

### `ResolveCurrentMobAdmissionAllowedDestroyed`
- From: `Destroyed`
- On: `ResolveCurrentMobAdmission`(can_manage_mob)
- Guards:
  - `manage_scope_allows`
- Emits: `CurrentMobAdmissionResolved`
- To: `Destroyed`

### `ResolveCurrentMobAdmissionDeniedRunning`
- From: `Running`
- On: `ResolveCurrentMobAdmission`(can_manage_mob)
- Guards:
  - `no_manage_scope_denies`
- Emits: `CurrentMobAdmissionResolved`
- To: `Running`

### `ResolveCurrentMobAdmissionDeniedStopped`
- From: `Stopped`
- On: `ResolveCurrentMobAdmission`(can_manage_mob)
- Guards:
  - `no_manage_scope_denies`
- Emits: `CurrentMobAdmissionResolved`
- To: `Stopped`

### `ResolveCurrentMobAdmissionDeniedCompleted`
- From: `Completed`
- On: `ResolveCurrentMobAdmission`(can_manage_mob)
- Guards:
  - `no_manage_scope_denies`
- Emits: `CurrentMobAdmissionResolved`
- To: `Completed`

### `ResolveCurrentMobAdmissionDeniedDestroyed`
- From: `Destroyed`
- On: `ResolveCurrentMobAdmission`(can_manage_mob)
- Guards:
  - `no_manage_scope_denies`
- Emits: `CurrentMobAdmissionResolved`
- To: `Destroyed`

### `ResolveSpawnToolAdmissionAllowedRunning`
- From: `Running`
- On: `ResolveSpawnToolAdmission`(can_manage_mob, spawn_profile_scope_present)
- Guards:
  - `spawn_scope_allows`
- Emits: `SpawnToolAdmissionResolved`
- To: `Running`

### `ResolveSpawnToolAdmissionAllowedStopped`
- From: `Stopped`
- On: `ResolveSpawnToolAdmission`(can_manage_mob, spawn_profile_scope_present)
- Guards:
  - `spawn_scope_allows`
- Emits: `SpawnToolAdmissionResolved`
- To: `Stopped`

### `ResolveSpawnToolAdmissionAllowedCompleted`
- From: `Completed`
- On: `ResolveSpawnToolAdmission`(can_manage_mob, spawn_profile_scope_present)
- Guards:
  - `spawn_scope_allows`
- Emits: `SpawnToolAdmissionResolved`
- To: `Completed`

### `ResolveSpawnToolAdmissionAllowedDestroyed`
- From: `Destroyed`
- On: `ResolveSpawnToolAdmission`(can_manage_mob, spawn_profile_scope_present)
- Guards:
  - `spawn_scope_allows`
- Emits: `SpawnToolAdmissionResolved`
- To: `Destroyed`

### `ResolveSpawnToolAdmissionDeniedRunning`
- From: `Running`
- On: `ResolveSpawnToolAdmission`(can_manage_mob, spawn_profile_scope_present)
- Guards:
  - `no_spawn_scope_denies`
- Emits: `SpawnToolAdmissionResolved`
- To: `Running`

### `ResolveSpawnToolAdmissionDeniedStopped`
- From: `Stopped`
- On: `ResolveSpawnToolAdmission`(can_manage_mob, spawn_profile_scope_present)
- Guards:
  - `no_spawn_scope_denies`
- Emits: `SpawnToolAdmissionResolved`
- To: `Stopped`

### `ResolveSpawnToolAdmissionDeniedCompleted`
- From: `Completed`
- On: `ResolveSpawnToolAdmission`(can_manage_mob, spawn_profile_scope_present)
- Guards:
  - `no_spawn_scope_denies`
- Emits: `SpawnToolAdmissionResolved`
- To: `Completed`

### `ResolveSpawnToolAdmissionDeniedDestroyed`
- From: `Destroyed`
- On: `ResolveSpawnToolAdmission`(can_manage_mob, spawn_profile_scope_present)
- Guards:
  - `no_spawn_scope_denies`
- Emits: `SpawnToolAdmissionResolved`
- To: `Destroyed`

### `ResolveCreateMobAdmissionAllowedRunning`
- From: `Running`
- On: `ResolveCreateMobAdmission`(can_create_mobs)
- Guards:
  - `create_capability_allows`
- Emits: `CreateMobAdmissionResolved`
- To: `Running`

### `ResolveCreateMobAdmissionAllowedStopped`
- From: `Stopped`
- On: `ResolveCreateMobAdmission`(can_create_mobs)
- Guards:
  - `create_capability_allows`
- Emits: `CreateMobAdmissionResolved`
- To: `Stopped`

### `ResolveCreateMobAdmissionAllowedCompleted`
- From: `Completed`
- On: `ResolveCreateMobAdmission`(can_create_mobs)
- Guards:
  - `create_capability_allows`
- Emits: `CreateMobAdmissionResolved`
- To: `Completed`

### `ResolveCreateMobAdmissionAllowedDestroyed`
- From: `Destroyed`
- On: `ResolveCreateMobAdmission`(can_create_mobs)
- Guards:
  - `create_capability_allows`
- Emits: `CreateMobAdmissionResolved`
- To: `Destroyed`

### `ResolveCreateMobAdmissionDeniedRunning`
- From: `Running`
- On: `ResolveCreateMobAdmission`(can_create_mobs)
- Guards:
  - `no_create_capability_denies`
- Emits: `CreateMobAdmissionResolved`
- To: `Running`

### `ResolveCreateMobAdmissionDeniedStopped`
- From: `Stopped`
- On: `ResolveCreateMobAdmission`(can_create_mobs)
- Guards:
  - `no_create_capability_denies`
- Emits: `CreateMobAdmissionResolved`
- To: `Stopped`

### `ResolveCreateMobAdmissionDeniedCompleted`
- From: `Completed`
- On: `ResolveCreateMobAdmission`(can_create_mobs)
- Guards:
  - `no_create_capability_denies`
- Emits: `CreateMobAdmissionResolved`
- To: `Completed`

### `ResolveCreateMobAdmissionDeniedDestroyed`
- From: `Destroyed`
- On: `ResolveCreateMobAdmission`(can_create_mobs)
- Guards:
  - `no_create_capability_denies`
- Emits: `CreateMobAdmissionResolved`
- To: `Destroyed`

### `InitializeAdaptiveRunRunningRunning`
- From: `Running`
- On: `InitializeAdaptiveRun`(adaptive_run_id, max_depth, max_total_decisions, max_repair_attempts, max_layer_failures, max_attempts_per_layer, max_members_per_layer, max_total_spawned_members, max_active_members, max_retained_layer_mobs, max_aggregate_tokens, max_aggregate_tool_calls, allowed_model_classes, allowed_tool_classes, allowed_skill_identities, allowed_auth_binding_refs, deadline_ms)
- Guards:
  - `no_active_adaptive_run`
  - `limits_complete`
- Emits: `AdaptiveRunInitialized`
- To: `Running`

### `RecordAdaptivePlanningDecisionActiveRunning`
- From: `Running`
- On: `RecordPlanningDecision`(adaptive_run_id, decision_kind)
- Guards:
  - `run_active`
  - `decision_limit_available`
- Emits: `AdaptivePlanningDecisionRecorded`
- To: `Running`

### `RecordAdaptivePlanningDecisionPlanLimitRunning`
- From: `Running`
- On: `RecordPlanningDecision`(adaptive_run_id, decision_kind)
- Guards:
  - `run_active`
  - `decision_limit_exhausted`
- Emits: `AdaptiveRunTerminalized`
- To: `Running`

### `RecordAdaptivePlanRejectedActiveRunning`
- From: `Running`
- On: `RecordPlanRejected`(adaptive_run_id, layer_id)
- Guards:
  - `run_active`
  - `repair_limit_available`
- Emits: `AdaptivePlanRejected`
- To: `Running`

### `RecordAdaptivePlanRejectedRepairLimitRunning`
- From: `Running`
- On: `RecordPlanRejected`(adaptive_run_id, layer_id)
- Guards:
  - `run_active`
  - `repair_limit_exhausted`
- Emits: `AdaptiveRunTerminalized`
- To: `Running`

### `ResolveAdaptiveLayerAdmissionAllowedRunning`
- From: `Running`
- On: `ResolveLayerAdmission`(adaptive_run_id, layer_id, attempt, plan_digest, child_mob_id, member_count, token_reservation, tool_call_reservation, used_model_classes, used_tool_classes, used_skill_identities, used_auth_binding_refs, observed_at_ms)
- Guards:
  - `run_active`
  - `deadline_available`
  - `depth_available`
  - `failure_limit_available`
  - `member_count_available`
  - `total_spawn_available`
  - `active_member_capacity_available`
  - `token_reservation_available`
  - `tool_call_reservation_available`
  - `model_classes_allowed`
  - `tool_classes_allowed`
  - `skill_identities_allowed`
  - `auth_bindings_allowed`
  - `no_active_layer`
  - `layer_not_previously_admitted`
- Emits: `AdaptiveLayerAdmissionResolved`
- To: `Running`

### `ResolveAdaptiveLayerAdmissionRejectedRunning`
- From: `Running`
- On: `ResolveLayerAdmission`(adaptive_run_id, layer_id, attempt, plan_digest, child_mob_id, member_count, token_reservation, tool_call_reservation, used_model_classes, used_tool_classes, used_skill_identities, used_auth_binding_refs, observed_at_ms)
- Guards:
  - `run_active`
  - `limit_exceeded`
- Emits: `AdaptiveLayerAdmissionResolved`
- To: `Running`

### `RecordAdaptiveLayerProvisionedRunning`
- From: `Running`
- On: `RecordLayerProvisioned`(adaptive_run_id, layer_id, attempt)
- Guards:
  - `layer_admitted`
  - `attempt_matches`
- Emits: `AdaptiveLayerProvisioned`
- To: `Running`

### `RecordAdaptiveLayerRunStartedRunning`
- From: `Running`
- On: `RecordLayerRunStarted`(adaptive_run_id, layer_id, attempt, child_run_id)
- Guards:
  - `layer_provisioning`
  - `attempt_matches`
- Emits: `AdaptiveLayerRunStarted`
- To: `Running`

### `IngestAdaptiveLayerTerminalSuccessRunning`
- From: `Running`
- On: `IngestLayerTerminal`(adaptive_run_id, layer_id, attempt, result_class, actual_tokens, actual_tool_calls)
- Guards:
  - `layer_running`
  - `attempt_matches`
  - `result_success`
- Emits: `AdaptiveLayerTerminalIngested`
- To: `Running`

### `IngestAdaptiveLayerTerminalFailureRunning`
- From: `Running`
- On: `IngestLayerTerminal`(adaptive_run_id, layer_id, attempt, result_class, actual_tokens, actual_tool_calls)
- Guards:
  - `layer_running`
  - `attempt_matches`
  - `result_failure`
- Emits: `AdaptiveLayerTerminalIngested`
- To: `Running`

### `RecordAdaptiveLayerSetupFaultRunning`
- From: `Running`
- On: `RecordLayerSetupFault`(adaptive_run_id, layer_id, attempt, fault, spawned_members, requested_members)
- Guards:
  - `layer_admitted_or_provisioning`
  - `attempt_matches`
  - `setup_counts_valid`
- Emits: `AdaptiveLayerSetupFaultRecorded`
- To: `Running`

### `RecordAdaptiveLayerResultValidatedRunning`
- From: `Running`
- On: `RecordLayerResultValidated`(adaptive_run_id, layer_id, attempt, result_digest)
- Guards:
  - `layer_collecting`
  - `attempt_matches`
- Emits: `AdaptiveLayerResultValidated`
- To: `Running`

### `RecordAdaptiveLayerResultInvalidRunning`
- From: `Running`
- On: `RecordLayerResultInvalid`(adaptive_run_id, layer_id, attempt)
- Guards:
  - `layer_collecting`
  - `attempt_matches`
- Emits: `AdaptiveLayerResultInvalid`
- To: `Running`

### `RecordAdaptiveLayerMobDestroyedRunning`
- From: `Running`
- On: `RecordLayerMobDestroyed`(adaptive_run_id, layer_id, attempt)
- Guards:
  - `layer_terminal`
  - `attempt_matches`
  - `active_member_debit_present`
- Emits: `AdaptiveLayerCleanupObserved`
- To: `Running`

### `RecordAdaptiveLayerMobRetainedRunning`
- From: `Running`
- On: `RecordLayerMobRetained`(adaptive_run_id, layer_id, attempt, disposition)
- Guards:
  - `layer_terminal`
  - `attempt_matches`
  - `retention_capacity_available`
  - `active_member_debit_present`
- Emits: `AdaptiveLayerCleanupObserved`
- To: `Running`

### `RecordAdaptiveLayerMobRetainedCleanupRequiredRunning`
- From: `Running`
- On: `RecordLayerMobRetained`(adaptive_run_id, layer_id, attempt, disposition)
- Guards:
  - `layer_terminal`
  - `attempt_matches`
  - `retention_capacity_exhausted`
  - `active_member_debit_present`
- Emits: `AdaptiveLayerCleanupObserved`
- To: `Running`

### `RecordAdaptiveCleanupResolvedRunning`
- From: `Running`
- On: `RecordCleanupResolved`(adaptive_run_id)
- Guards:
  - `cleanup_pause_active`
- Emits: `AdaptiveCleanupResolved`
- To: `Running`

### `RecordAdaptiveBodyEvidenceMissingRunning`
- From: `Running`
- On: `RecordBodyEvidenceMissing`(adaptive_run_id, missing_digest)
- Guards:
  - `run_active`
- Emits: `AdaptiveBodyEvidenceMissing`
- To: `Running`

### `ResolveAdaptiveFinishRunningRunning`
- From: `Running`
- On: `ResolveAdaptiveFinish`(adaptive_run_id, final_result_digest)
- Guards:
  - `run_active`
- Emits: `AdaptiveRunTerminalized`
- To: `Running`

### `RequestAdaptiveCancelRunningRunning`
- From: `Running`
- On: `RequestAdaptiveCancel`(adaptive_run_id)
- Guards:
  - `run_active`
- Emits: `AdaptiveRunTerminalized`
- To: `Running`

### `RecordAdaptiveDeadlineObservedExpiredRunning`
- From: `Running`
- On: `RecordDeadlineObserved`(adaptive_run_id, observed_at_ms)
- Guards:
  - `run_active`
  - `deadline_expired`
- Emits: `AdaptiveRunTerminalized`
- To: `Running`

### `ResolveProfileMutationAdmissionAllowedRunning`
- From: `Running`
- On: `ResolveProfileMutationAdmission`(can_mutate_profiles)
- Guards:
  - `mutate_capability_allows`
- Emits: `ProfileMutationAdmissionResolved`
- To: `Running`

### `ResolveProfileMutationAdmissionAllowedStopped`
- From: `Stopped`
- On: `ResolveProfileMutationAdmission`(can_mutate_profiles)
- Guards:
  - `mutate_capability_allows`
- Emits: `ProfileMutationAdmissionResolved`
- To: `Stopped`

### `ResolveProfileMutationAdmissionAllowedCompleted`
- From: `Completed`
- On: `ResolveProfileMutationAdmission`(can_mutate_profiles)
- Guards:
  - `mutate_capability_allows`
- Emits: `ProfileMutationAdmissionResolved`
- To: `Completed`

### `ResolveProfileMutationAdmissionAllowedDestroyed`
- From: `Destroyed`
- On: `ResolveProfileMutationAdmission`(can_mutate_profiles)
- Guards:
  - `mutate_capability_allows`
- Emits: `ProfileMutationAdmissionResolved`
- To: `Destroyed`

### `ResolveProfileMutationAdmissionDeniedRunning`
- From: `Running`
- On: `ResolveProfileMutationAdmission`(can_mutate_profiles)
- Guards:
  - `no_mutate_capability_denies`
- Emits: `ProfileMutationAdmissionResolved`
- To: `Running`

### `ResolveProfileMutationAdmissionDeniedStopped`
- From: `Stopped`
- On: `ResolveProfileMutationAdmission`(can_mutate_profiles)
- Guards:
  - `no_mutate_capability_denies`
- Emits: `ProfileMutationAdmissionResolved`
- To: `Stopped`

### `ResolveProfileMutationAdmissionDeniedCompleted`
- From: `Completed`
- On: `ResolveProfileMutationAdmission`(can_mutate_profiles)
- Guards:
  - `no_mutate_capability_denies`
- Emits: `ProfileMutationAdmissionResolved`
- To: `Completed`

### `ResolveProfileMutationAdmissionDeniedDestroyed`
- From: `Destroyed`
- On: `ResolveProfileMutationAdmission`(can_mutate_profiles)
- Guards:
  - `no_mutate_capability_denies`
- Emits: `ProfileMutationAdmissionResolved`
- To: `Destroyed`

### `ClassifyMemberOperationEligibilityAdmittedRunning`
- From: `Running`
- On: `ClassifyMemberOperationEligibility`()
- Guards:
  - `running_and_not_destroying_is_eligible`
- Emits: `MemberOperationEligibilityResolved`
- To: `Running`

### `ClassifyMemberOperationEligibilityRunningDestroyDeniedRunning`
- From: `Running`
- On: `ClassifyMemberOperationEligibility`()
- Guards:
  - `running_but_destroying_is_denied`
- Emits: `MemberOperationEligibilityResolved`
- To: `Running`

### `ClassifyMemberOperationEligibilityNotRunningStopped`
- From: `Stopped`
- On: `ClassifyMemberOperationEligibility`()
- Emits: `MemberOperationEligibilityResolved`
- To: `Stopped`

### `ClassifyMemberOperationEligibilityNotRunningCompleted`
- From: `Completed`
- On: `ClassifyMemberOperationEligibility`()
- Emits: `MemberOperationEligibilityResolved`
- To: `Completed`

### `ClassifyMemberOperationEligibilityNotRunningDestroyed`
- From: `Destroyed`
- On: `ClassifyMemberOperationEligibility`()
- Emits: `MemberOperationEligibilityResolved`
- To: `Destroyed`

### `ClassifyBridgeRejectionRecoveryRebindRunning`
- From: `Running`
- On: `ClassifyBridgeRejectionRecovery`(rejection_cause)
- Guards:
  - `rejection_recoverable_by_rebind`
- Emits: `BridgeRejectionRecoveryClassified`
- To: `Running`

### `ClassifyBridgeRejectionRecoveryRebindStopped`
- From: `Stopped`
- On: `ClassifyBridgeRejectionRecovery`(rejection_cause)
- Guards:
  - `rejection_recoverable_by_rebind`
- Emits: `BridgeRejectionRecoveryClassified`
- To: `Stopped`

### `ClassifyBridgeRejectionRecoveryRebindCompleted`
- From: `Completed`
- On: `ClassifyBridgeRejectionRecovery`(rejection_cause)
- Guards:
  - `rejection_recoverable_by_rebind`
- Emits: `BridgeRejectionRecoveryClassified`
- To: `Completed`

### `ClassifyBridgeRejectionRecoveryRebindDestroyed`
- From: `Destroyed`
- On: `ClassifyBridgeRejectionRecovery`(rejection_cause)
- Guards:
  - `rejection_recoverable_by_rebind`
- Emits: `BridgeRejectionRecoveryClassified`
- To: `Destroyed`

### `ClassifyBridgeRejectionRecoveryFatalRunning`
- From: `Running`
- On: `ClassifyBridgeRejectionRecovery`(rejection_cause)
- Guards:
  - `rejection_fatal`
- Emits: `BridgeRejectionRecoveryClassified`
- To: `Running`

### `ClassifyBridgeRejectionRecoveryFatalStopped`
- From: `Stopped`
- On: `ClassifyBridgeRejectionRecovery`(rejection_cause)
- Guards:
  - `rejection_fatal`
- Emits: `BridgeRejectionRecoveryClassified`
- To: `Stopped`

### `ClassifyBridgeRejectionRecoveryFatalCompleted`
- From: `Completed`
- On: `ClassifyBridgeRejectionRecovery`(rejection_cause)
- Guards:
  - `rejection_fatal`
- Emits: `BridgeRejectionRecoveryClassified`
- To: `Completed`

### `ClassifyBridgeRejectionRecoveryFatalDestroyed`
- From: `Destroyed`
- On: `ClassifyBridgeRejectionRecovery`(rejection_cause)
- Guards:
  - `rejection_fatal`
- Emits: `BridgeRejectionRecoveryClassified`
- To: `Destroyed`

### `ClassifyPendingSupervisorAcceptanceNotConfirmedRunning`
- From: `Running`
- On: `ClassifyPendingSupervisorAcceptance`(rejection_cause)
- Guards:
  - `pending_acceptance_not_confirmed_reattempt`
- Emits: `PendingSupervisorAcceptanceClassified`
- To: `Running`

### `ClassifyPendingSupervisorAcceptanceNotConfirmedStopped`
- From: `Stopped`
- On: `ClassifyPendingSupervisorAcceptance`(rejection_cause)
- Guards:
  - `pending_acceptance_not_confirmed_reattempt`
- Emits: `PendingSupervisorAcceptanceClassified`
- To: `Stopped`

### `ClassifyPendingSupervisorAcceptanceNotConfirmedCompleted`
- From: `Completed`
- On: `ClassifyPendingSupervisorAcceptance`(rejection_cause)
- Guards:
  - `pending_acceptance_not_confirmed_reattempt`
- Emits: `PendingSupervisorAcceptanceClassified`
- To: `Completed`

### `ClassifyPendingSupervisorAcceptanceNotConfirmedDestroyed`
- From: `Destroyed`
- On: `ClassifyPendingSupervisorAcceptance`(rejection_cause)
- Guards:
  - `pending_acceptance_not_confirmed_reattempt`
- Emits: `PendingSupervisorAcceptanceClassified`
- To: `Destroyed`

### `ClassifyPendingSupervisorAcceptanceStaleRunning`
- From: `Running`
- On: `ClassifyPendingSupervisorAcceptance`(rejection_cause)
- Guards:
  - `pending_acceptance_stale_authority`
- Emits: `PendingSupervisorAcceptanceClassified`
- To: `Running`

### `ClassifyPendingSupervisorAcceptanceStaleStopped`
- From: `Stopped`
- On: `ClassifyPendingSupervisorAcceptance`(rejection_cause)
- Guards:
  - `pending_acceptance_stale_authority`
- Emits: `PendingSupervisorAcceptanceClassified`
- To: `Stopped`

### `ClassifyPendingSupervisorAcceptanceStaleCompleted`
- From: `Completed`
- On: `ClassifyPendingSupervisorAcceptance`(rejection_cause)
- Guards:
  - `pending_acceptance_stale_authority`
- Emits: `PendingSupervisorAcceptanceClassified`
- To: `Completed`

### `ClassifyPendingSupervisorAcceptanceStaleDestroyed`
- From: `Destroyed`
- On: `ClassifyPendingSupervisorAcceptance`(rejection_cause)
- Guards:
  - `pending_acceptance_stale_authority`
- Emits: `PendingSupervisorAcceptanceClassified`
- To: `Destroyed`

### `ClassifyPendingSupervisorAcceptanceFatalRunning`
- From: `Running`
- On: `ClassifyPendingSupervisorAcceptance`(rejection_cause)
- Guards:
  - `pending_acceptance_fatal`
- Emits: `PendingSupervisorAcceptanceClassified`
- To: `Running`

### `ClassifyPendingSupervisorAcceptanceFatalStopped`
- From: `Stopped`
- On: `ClassifyPendingSupervisorAcceptance`(rejection_cause)
- Guards:
  - `pending_acceptance_fatal`
- Emits: `PendingSupervisorAcceptanceClassified`
- To: `Stopped`

### `ClassifyPendingSupervisorAcceptanceFatalCompleted`
- From: `Completed`
- On: `ClassifyPendingSupervisorAcceptance`(rejection_cause)
- Guards:
  - `pending_acceptance_fatal`
- Emits: `PendingSupervisorAcceptanceClassified`
- To: `Completed`

### `ClassifyPendingSupervisorAcceptanceFatalDestroyed`
- From: `Destroyed`
- On: `ClassifyPendingSupervisorAcceptance`(rejection_cause)
- Guards:
  - `pending_acceptance_fatal`
- Emits: `PendingSupervisorAcceptanceClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureProfileNotFoundRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `profile_not_found`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureProfileNotFoundStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `profile_not_found`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureProfileNotFoundCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `profile_not_found`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureProfileNotFoundDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `profile_not_found`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureMemberNotFoundRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `member_not_found`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureMemberNotFoundStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `member_not_found`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureMemberNotFoundCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `member_not_found`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureMemberNotFoundDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `member_not_found`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureMemberAlreadyExistsRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `member_already_exists`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureMemberAlreadyExistsStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `member_already_exists`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureMemberAlreadyExistsCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `member_already_exists`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureMemberAlreadyExistsDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `member_already_exists`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureNotExternallyAddressableRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `not_externally_addressable`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureNotExternallyAddressableStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `not_externally_addressable`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureNotExternallyAddressableCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `not_externally_addressable`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureNotExternallyAddressableDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `not_externally_addressable`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureInvalidTransitionRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `invalid_transition`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureInvalidTransitionStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `invalid_transition`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureInvalidTransitionCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `invalid_transition`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureInvalidTransitionDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `invalid_transition`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureWiringErrorRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `wiring_error`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureWiringErrorStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `wiring_error`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureWiringErrorCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `wiring_error`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureWiringErrorDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `wiring_error`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureBridgeCommandRejectedRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `bridge_command_rejected`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureBridgeCommandRejectedStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `bridge_command_rejected`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureBridgeCommandRejectedCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `bridge_command_rejected`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureBridgeCommandRejectedDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `bridge_command_rejected`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureMemberRestoreFailedRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `member_restore_failed`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureMemberRestoreFailedStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `member_restore_failed`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureMemberRestoreFailedCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `member_restore_failed`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureMemberRestoreFailedDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `member_restore_failed`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureKickoffWaitTimedOutRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `kickoff_wait_timed_out`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureKickoffWaitTimedOutStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `kickoff_wait_timed_out`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureKickoffWaitTimedOutCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `kickoff_wait_timed_out`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureKickoffWaitTimedOutDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `kickoff_wait_timed_out`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureReadyWaitTimedOutRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `ready_wait_timed_out`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureReadyWaitTimedOutStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `ready_wait_timed_out`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureReadyWaitTimedOutCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `ready_wait_timed_out`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureReadyWaitTimedOutDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `ready_wait_timed_out`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureDefinitionErrorRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `definition_error`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureDefinitionErrorStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `definition_error`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureDefinitionErrorCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `definition_error`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureDefinitionErrorDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `definition_error`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureFlowNotFoundRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `flow_not_found`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureFlowNotFoundStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `flow_not_found`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureFlowNotFoundCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `flow_not_found`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureFlowNotFoundDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `flow_not_found`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureFlowFailedRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `flow_failed`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureFlowFailedStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `flow_failed`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureFlowFailedCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `flow_failed`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureFlowFailedDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `flow_failed`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureRunNotFoundRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `run_not_found`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureRunNotFoundStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `run_not_found`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureRunNotFoundCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `run_not_found`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureRunNotFoundDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `run_not_found`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureRunCanceledRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `run_canceled`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureRunCanceledStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `run_canceled`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureRunCanceledCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `run_canceled`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureRunCanceledDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `run_canceled`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureFlowTurnTimedOutRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `flow_turn_timed_out`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureFlowTurnTimedOutStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `flow_turn_timed_out`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureFlowTurnTimedOutCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `flow_turn_timed_out`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureFlowTurnTimedOutDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `flow_turn_timed_out`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureFrameDepthLimitExceededRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `frame_depth_limit_exceeded`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureFrameDepthLimitExceededStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `frame_depth_limit_exceeded`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureFrameDepthLimitExceededCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `frame_depth_limit_exceeded`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureFrameDepthLimitExceededDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `frame_depth_limit_exceeded`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureFrameAtomicPersistenceUnavailableRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `frame_atomic_persistence_unavailable`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureFrameAtomicPersistenceUnavailableStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `frame_atomic_persistence_unavailable`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureFrameAtomicPersistenceUnavailableCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `frame_atomic_persistence_unavailable`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureFrameAtomicPersistenceUnavailableDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `frame_atomic_persistence_unavailable`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureSpecRevisionConflictRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `spec_revision_conflict`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureSpecRevisionConflictStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `spec_revision_conflict`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureSpecRevisionConflictCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `spec_revision_conflict`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureSpecRevisionConflictDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `spec_revision_conflict`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureSchemaValidationRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `schema_validation`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureSchemaValidationStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `schema_validation`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureSchemaValidationCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `schema_validation`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureSchemaValidationDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `schema_validation`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureInsufficientTargetsRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `insufficient_targets`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureInsufficientTargetsStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `insufficient_targets`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureInsufficientTargetsCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `insufficient_targets`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureInsufficientTargetsDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `insufficient_targets`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureTopologyViolationRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `topology_violation`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureTopologyViolationStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `topology_violation`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureTopologyViolationCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `topology_violation`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureTopologyViolationDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `topology_violation`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureBridgeDeliveryRejectedRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `bridge_delivery_rejected`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureBridgeDeliveryRejectedStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `bridge_delivery_rejected`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureBridgeDeliveryRejectedCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `bridge_delivery_rejected`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureBridgeDeliveryRejectedDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `bridge_delivery_rejected`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureSupervisorEscalationRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `supervisor_escalation`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureSupervisorEscalationStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `supervisor_escalation`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureSupervisorEscalationCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `supervisor_escalation`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureSupervisorEscalationDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `supervisor_escalation`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureUnsupportedForModeRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `unsupported_for_mode`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureUnsupportedForModeStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `unsupported_for_mode`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureUnsupportedForModeCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `unsupported_for_mode`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureUnsupportedForModeDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `unsupported_for_mode`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureMissingMemberCapabilityRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `missing_member_capability`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureMissingMemberCapabilityStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `missing_member_capability`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureMissingMemberCapabilityCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `missing_member_capability`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureMissingMemberCapabilityDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `missing_member_capability`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureResetBarrierRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `reset_barrier`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureResetBarrierStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `reset_barrier`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureResetBarrierCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `reset_barrier`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureResetBarrierDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `reset_barrier`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureStorageErrorRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `storage_error`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureStorageErrorStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `storage_error`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureStorageErrorCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `storage_error`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureStorageErrorDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `storage_error`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureSessionErrorRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `session_error`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureSessionErrorStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `session_error`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureSessionErrorCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `session_error`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureSessionErrorDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `session_error`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureCommsErrorRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `comms_error`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureCommsErrorStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `comms_error`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureCommsErrorCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `comms_error`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureCommsErrorDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `comms_error`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureCallbackPendingRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `callback_pending`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureCallbackPendingStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `callback_pending`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureCallbackPendingCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `callback_pending`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureCallbackPendingDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `callback_pending`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureStaleFenceTokenRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `stale_fence_token`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureStaleFenceTokenStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `stale_fence_token`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureStaleFenceTokenCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `stale_fence_token`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureStaleFenceTokenDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `stale_fence_token`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureStaleEventCursorRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `stale_event_cursor`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureStaleEventCursorStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `stale_event_cursor`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureStaleEventCursorCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `stale_event_cursor`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureStaleEventCursorDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `stale_event_cursor`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureWorkNotFoundRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `work_not_found`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureWorkNotFoundStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `work_not_found`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureWorkNotFoundCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `work_not_found`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureWorkNotFoundDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `work_not_found`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `ClassifySpawnManyFailureInternalRunning`
- From: `Running`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `internal`
- Emits: `SpawnManyFailureClassified`
- To: `Running`

### `ClassifySpawnManyFailureInternalStopped`
- From: `Stopped`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `internal`
- Emits: `SpawnManyFailureClassified`
- To: `Stopped`

### `ClassifySpawnManyFailureInternalCompleted`
- From: `Completed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `internal`
- Emits: `SpawnManyFailureClassified`
- To: `Completed`

### `ClassifySpawnManyFailureInternalDestroyed`
- From: `Destroyed`
- On: `ClassifySpawnManyFailure`(observation)
- Guards:
  - `internal`
- Emits: `SpawnManyFailureClassified`
- To: `Destroyed`

### `CommitSpawnMembershipFresh`
- From: `Running`
- On: `CommitSpawnMembership`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing)
- Guards:
  - `spawn_exec_opened`
  - `no_prior_session_binding`
  - `replacing_absent`
  - `bridge_session_present`
- Emits: `RequestRuntimeBinding`, `AppendLifecycleJournal`, `MemberSessionBindingChanged`, `EmitMemberLifecycleNotice`
- To: `Running`

### `CommitSpawnMembershipFreshPeerOnly`
- From: `Running`
- On: `CommitSpawnMembership`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing)
- Guards:
  - `spawn_exec_opened`
  - `no_prior_session_binding`
  - `replacing_absent`
  - `bridge_session_absent`
- Emits: `AppendLifecycleJournal`, `EmitMemberLifecycleNotice`
- To: `Running`

### `CommitSpawnMembershipReplacing`
- From: `Running`
- On: `CommitSpawnMembership`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing)
- Guards:
  - `spawn_exec_opened`
  - `prior_session_binding_present`
  - `replacing_present`
  - `replacing_matches_current`
  - `bridge_session_present`
- Emits: `RequestRuntimeBinding`, `AppendLifecycleJournal`, `MemberSessionBindingChanged`, `EmitMemberLifecycleNotice`
- To: `Running`

### `BeginSpawnExecFresh`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing)
- Guards:
  - `coordinator_bound`
  - `spawn_exec_settled`
  - `no_prior_session_binding`
  - `replacing_absent`
  - `identity_not_external_peer`
  - `spawn_profile_name_authorized`
  - `spawn_profile_authorized`
  - `spawn_profile_addressability_authorized`
  - `bridge_session_present`
- To: `Running`

### `BeginSpawnExecFreshPeerOnly`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing)
- Guards:
  - `coordinator_bound`
  - `spawn_exec_settled`
  - `no_prior_session_binding`
  - `replacing_absent`
  - `identity_not_external_peer`
  - `spawn_profile_name_authorized`
  - `spawn_profile_authorized`
  - `spawn_profile_addressability_authorized`
  - `bridge_session_absent`
- To: `Running`

### `BeginSpawnExecReplacing`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing)
- Guards:
  - `coordinator_bound`
  - `spawn_exec_settled`
  - `prior_session_binding_present`
  - `replacing_present`
  - `replacing_matches_current`
  - `identity_not_external_peer`
  - `spawn_profile_name_authorized`
  - `spawn_profile_authorized`
  - `spawn_profile_addressability_authorized`
  - `bridge_session_present`
- To: `Running`

### `CommitSpawnActivationFinalRunning`
- From: `Running`
- On: `CommitSpawnActivation`(agent_identity)
- Guards:
  - `spawn_exec_membership_committed`
- To: `Running`

### `CommitSpawnActivationFinalStopped`
- From: `Stopped`
- On: `CommitSpawnActivation`(agent_identity)
- Guards:
  - `spawn_exec_membership_committed`
- To: `Stopped`

### `CommitSpawnActivationFinalCompleted`
- From: `Completed`
- On: `CommitSpawnActivation`(agent_identity)
- Guards:
  - `spawn_exec_membership_committed`
- To: `Completed`

### `CommitSpawnActivationLateArrivalRunning`
- From: `Running`
- On: `CommitSpawnActivation`(agent_identity)
- Guards:
  - `not_committed`
- To: `Running`

### `CommitSpawnActivationLateArrivalStopped`
- From: `Stopped`
- On: `CommitSpawnActivation`(agent_identity)
- Guards:
  - `not_committed`
- To: `Stopped`

### `CommitSpawnActivationLateArrivalCompleted`
- From: `Completed`
- On: `CommitSpawnActivation`(agent_identity)
- Guards:
  - `not_committed`
- To: `Completed`

### `CommitSpawnActivationDestroyed`
- From: `Destroyed`
- On: `CommitSpawnActivation`(agent_identity)
- To: `Destroyed`

### `AbortSpawnExecActiveRunning`
- From: `Running`
- On: `AbortSpawnExec`(agent_identity)
- Guards:
  - `spawn_exec_in_flight`
- To: `Running`

### `AbortSpawnExecActiveStopped`
- From: `Stopped`
- On: `AbortSpawnExec`(agent_identity)
- Guards:
  - `spawn_exec_in_flight`
- To: `Stopped`

### `AbortSpawnExecActiveCompleted`
- From: `Completed`
- On: `AbortSpawnExec`(agent_identity)
- Guards:
  - `spawn_exec_in_flight`
- To: `Completed`

### `AbortSpawnExecLateArrivalRunning`
- From: `Running`
- On: `AbortSpawnExec`(agent_identity)
- Guards:
  - `spawn_exec_settled`
- To: `Running`

### `AbortSpawnExecLateArrivalStopped`
- From: `Stopped`
- On: `AbortSpawnExec`(agent_identity)
- Guards:
  - `spawn_exec_settled`
- To: `Stopped`

### `AbortSpawnExecLateArrivalCompleted`
- From: `Completed`
- On: `AbortSpawnExec`(agent_identity)
- Guards:
  - `spawn_exec_settled`
- To: `Completed`

### `AbortSpawnExecDestroyed`
- From: `Destroyed`
- On: `AbortSpawnExec`(agent_identity)
- To: `Destroyed`

### `AuthorizeSpawnProfileRunning`
- From: `Running`
- On: `AuthorizeSpawnProfile`(agent_identity, profile_name, model, profile_material_digest, tool_config_digest, skills_digest, provider_params_digest, output_schema_digest, external_addressable)
- Guards:
  - `coordinator_bound`
- Emits: `SpawnProfileAuthorized`
- To: `Running`

### `EnsureMemberRunningExisting`
- From: `Running`
- On: `EnsureMember`(agent_identity)
- Guards:
  - `coordinator_bound`
  - `identity_present`
- Emits: `MemberRetainRequired`
- To: `Running`

### `EnsureMemberRunningMissing`
- From: `Running`
- On: `EnsureMember`(agent_identity)
- Guards:
  - `coordinator_bound`
  - `identity_absent`
- Emits: `MemberSpawnRequired`
- To: `Running`

### `RecoverRosterMemberRunning`
- From: `Running`
- On: `RecoverRosterMember`(agent_identity, agent_runtime_id, fence_token, generation, profile_name, runtime_mode, external_addressable)
- Guards:
  - `identity_not_recovered`
  - `runtime_not_recovered`
- To: `Running`

### `RecoverRosterMemberAddressabilityRunning`
- From: `Running`
- On: `RecoverRosterMember`(agent_identity, agent_runtime_id, fence_token, generation, profile_name, runtime_mode, external_addressable)
- Guards:
  - `identity_runtime_matches`
  - `runtime_recovered`
  - `fence_token_matches`
  - `generation_matches`
- To: `Running`

### `RecoverMemberSessionBindingFreshRunning`
- From: `Running`
- On: `RecoverMemberSessionBinding`(agent_identity, agent_runtime_id, bridge_session_id, replacing)
- Guards:
  - `identity_runtime_matches`
  - `runtime_recovered`
  - `no_prior_session_binding`
  - `replacing_absent`
- Emits: `MemberSessionBindingChanged`, `SessionProvisionOperationOwnerAuthorized`
- To: `Running`

### `RecoverMemberSessionBindingReplacingRunning`
- From: `Running`
- On: `RecoverMemberSessionBinding`(agent_identity, agent_runtime_id, bridge_session_id, replacing)
- Guards:
  - `identity_runtime_matches`
  - `runtime_recovered`
  - `prior_session_binding_present`
  - `replacing_present`
  - `replacing_matches_current`
  - `replacement_changes_binding`
- Emits: `MemberSessionBindingChanged`, `SessionProvisionOperationOwnerAuthorized`
- To: `Running`

### `RecoverMemberSessionBindingAlreadyCurrentRunning`
- From: `Running`
- On: `RecoverMemberSessionBinding`(agent_identity, agent_runtime_id, bridge_session_id, replacing)
- Guards:
  - `identity_runtime_matches`
  - `runtime_recovered`
  - `prior_session_binding_present`
  - `binding_already_current`
  - `replacing_absent_or_current`
- Emits: `SessionProvisionOperationOwnerAuthorized`
- To: `Running`

### `RecoverSpawnedMemberPeerEndpointFreshRunning`
- From: `Running`
- On: `RecoverSpawnedMemberPeerEndpoint`(agent_identity, agent_runtime_id, fence_token, generation, peer_endpoint)
- Guards:
  - `identity_runtime_matches`
  - `runtime_recovered`
  - `fence_token_matches`
  - `generation_matches`
  - `member_peer_not_registered`
  - `member_endpoint_not_registered`
  - `prior_endpoint_history_absent`
  - `peer_id_available_for_identity`
- Emits: `MemberPeerRegistered`
- To: `Running`

### `RecoverSpawnedMemberPeerEndpointAlreadyCurrentRunning`
- From: `Running`
- On: `RecoverSpawnedMemberPeerEndpoint`(agent_identity, agent_runtime_id, fence_token, generation, peer_endpoint)
- Guards:
  - `identity_runtime_matches`
  - `runtime_recovered`
  - `fence_token_matches`
  - `generation_matches`
  - `member_peer_registered`
  - `member_endpoint_registered`
  - `current_endpoint_matches`
  - `current_peer_id_matches`
- Emits: `MemberPeerRegistered`
- To: `Running`

### `RecoverMemberPeerEndpointFreshRunning`
- From: `Running`
- On: `RecoverMemberPeerEndpoint`(agent_identity, agent_runtime_id, bridge_session_id, peer_endpoint)
- Guards:
  - `identity_runtime_matches`
  - `runtime_recovered`
  - `member_not_retiring`
  - `session_binding_matches`
  - `member_peer_not_registered`
  - `member_endpoint_not_registered`
  - `prior_endpoint_history_absent`
  - `no_member_wiring_edges`
  - `no_external_peer_edges`
  - `peer_id_available_for_identity`
- Emits: `MemberPeerRegistered`
- To: `Running`

### `RecoverMemberPeerEndpointAlreadyCurrentRunning`
- From: `Running`
- On: `RecoverMemberPeerEndpoint`(agent_identity, agent_runtime_id, bridge_session_id, peer_endpoint)
- Guards:
  - `identity_runtime_matches`
  - `runtime_recovered`
  - `member_not_retiring`
  - `session_binding_matches`
  - `member_peer_registered`
  - `member_endpoint_registered`
  - `current_endpoint_matches_recovered`
  - `current_peer_id_matches_recovered`
- Emits: `MemberPeerRegistered`
- To: `Running`

### `RecoverMemberPeerEndpointChangedRunning`
- From: `Running`
- On: `RecoverMemberPeerEndpoint`(agent_identity, agent_runtime_id, bridge_session_id, peer_endpoint)
- Guards:
  - `identity_runtime_matches`
  - `runtime_recovered`
  - `member_not_retiring`
  - `session_binding_matches`
  - `member_peer_registered`
  - `member_endpoint_registered`
  - `current_peer_id_matches_endpoint`
  - `recovered_endpoint_changed`
  - `recovered_peer_id_changed`
  - `recovered_peer_id_available_for_identity`
  - `no_external_peer_edges`
- Emits: `MemberPeerRegistered`
- To: `Running`

### `RecoverRosterMemberResetRunning`
- From: `Running`
- On: `RecoverRosterMemberReset`(agent_identity, previous_agent_runtime_id, agent_runtime_id, fence_token, generation)
- Guards:
  - `previous_runtime_recovered`
  - `identity_recovered`
  - `previous_runtime_session_ingress_detach_closed`
  - `previous_runtime_not_retiring`
  - `previous_runtime_retirement_closed`
  - `previous_runtime_retire_refusal_closed`
- To: `Running`

### `RecoverRosterMemberRetirementStartedReleasing`
- From: `Running`
- On: `RecoverRosterMemberRetirementStarted`(agent_identity, agent_runtime_id, generation, releasing, session_id, retiring_peer_endpoint)
- Guards:
  - `runtime_recovered`
  - `identity_binding_matches`
  - `generation_matches`
  - `releasing_present`
  - `session_matches_releasing`
  - `binding_matches_releasing`
  - `retiring_peer_endpoint_consistent`
- To: `Running`

### `RecoverRosterMemberRetirementStartedReleasingAlreadyApplied`
- From: `Running`
- On: `RecoverRosterMemberRetirementStarted`(agent_identity, agent_runtime_id, generation, releasing, session_id, retiring_peer_endpoint)
- Guards:
  - `runtime_recovered`
  - `identity_binding_matches`
  - `generation_matches`
  - `releasing_present`
  - `session_matches_releasing`
  - `binding_already_released`
  - `member_already_retiring`
  - `retirement_session_already_pending`
  - `ingress_detach_still_pending`
  - `retiring_peer_endpoint_consistent`
- To: `Running`

### `RecoverRosterMemberRetirementStartedPreservingBinding`
- From: `Running`
- On: `RecoverRosterMemberRetirementStarted`(agent_identity, agent_runtime_id, generation, releasing, session_id, retiring_peer_endpoint)
- Guards:
  - `runtime_recovered`
  - `identity_binding_matches`
  - `generation_matches`
  - `releasing_absent`
  - `session_present`
  - `binding_matches_session`
  - `retiring_peer_endpoint_consistent`
- To: `Running`

### `RecoverRosterMemberRetirementStartedPeerOnly`
- From: `Running`
- On: `RecoverRosterMemberRetirementStarted`(agent_identity, agent_runtime_id, generation, releasing, session_id, retiring_peer_endpoint)
- Guards:
  - `runtime_recovered`
  - `identity_binding_matches`
  - `generation_matches`
  - `releasing_absent`
  - `session_absent`
  - `binding_absent`
  - `retiring_peer_endpoint_consistent`
- To: `Running`

### `RecoverRemoteMemberRuntimeRetiredFreshRunning`
- From: `Running`
- On: `RecoverRemoteMemberRuntimeRetired`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `remote_member_session_absent`
  - `remote_runtime_not_recorded`
- To: `Running`

### `RecoverRemoteMemberRuntimeRetiredFreshStopped`
- From: `Stopped`
- On: `RecoverRemoteMemberRuntimeRetired`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `remote_member_session_absent`
  - `remote_runtime_not_recorded`
- To: `Stopped`

### `RecoverRemoteMemberRuntimeRetiredAlreadyRecordedRunning`
- From: `Running`
- On: `RecoverRemoteMemberRuntimeRetired`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `remote_member_session_absent`
  - `remote_runtime_recorded`
- To: `Running`

### `RecoverRemoteMemberRuntimeRetiredAlreadyRecordedStopped`
- From: `Stopped`
- On: `RecoverRemoteMemberRuntimeRetired`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `remote_member_session_absent`
  - `remote_runtime_recorded`
- To: `Stopped`

### `RecoverRemoteMemberSupervisorRevokedFreshRunning`
- From: `Running`
- On: `RecoverRemoteMemberSupervisorRevoked`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `remote_member_session_absent`
  - `remote_runtime_recorded`
  - `remote_supervisor_not_recorded`
- To: `Running`

### `RecoverRemoteMemberSupervisorRevokedFreshStopped`
- From: `Stopped`
- On: `RecoverRemoteMemberSupervisorRevoked`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `remote_member_session_absent`
  - `remote_runtime_recorded`
  - `remote_supervisor_not_recorded`
- To: `Stopped`

### `RecoverRemoteMemberSupervisorRevokedAlreadyRecordedRunning`
- From: `Running`
- On: `RecoverRemoteMemberSupervisorRevoked`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `remote_member_session_absent`
  - `remote_runtime_recorded`
  - `remote_supervisor_recorded`
- To: `Running`

### `RecoverRemoteMemberSupervisorRevokedAlreadyRecordedStopped`
- From: `Stopped`
- On: `RecoverRemoteMemberSupervisorRevoked`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `remote_member_session_absent`
  - `remote_runtime_recorded`
  - `remote_supervisor_recorded`
- To: `Stopped`

### `RecoverRosterMemberRetiredRunning`
- From: `Running`
- On: `RecoverRosterMemberRetired`(agent_identity, agent_runtime_id)
- Guards:
  - `runtime_recovered`
  - `identity_binding_matches`
- To: `Running`

### `RecoverRosterMemberRetiredAlreadyAbsent`
- From: `Running`
- On: `RecoverRosterMemberRetired`(agent_identity, agent_runtime_id)
- Guards:
  - `identity_absent`
- To: `Running`

### `RecoverRosterMemberRetiredStaleGeneration`
- From: `Running`
- On: `RecoverRosterMemberRetired`(agent_identity, agent_runtime_id)
- Guards:
  - `identity_remapped`
- To: `Running`

### `RecoverMemberKickoffPending`
- From: `Running`
- On: `RecoverMemberKickoff`(member_id, phase, error)
- Guards:
  - `recover_pending_phase`
  - `recover_pending_without_error`
- To: `Running`

### `RecoverMemberKickoffStarting`
- From: `Running`
- On: `RecoverMemberKickoff`(member_id, phase, error)
- Guards:
  - `recover_starting_phase`
  - `recover_starting_without_error`
- To: `Running`

### `RecoverMemberKickoffCallbackPending`
- From: `Running`
- On: `RecoverMemberKickoff`(member_id, phase, error)
- Guards:
  - `recover_callback_pending_phase`
  - `recover_callback_pending_without_error`
- To: `Running`

### `RecoverMemberKickoffStarted`
- From: `Running`
- On: `RecoverMemberKickoff`(member_id, phase, error)
- Guards:
  - `recover_started_phase`
  - `recover_started_without_error`
- To: `Running`

### `RecoverMemberKickoffFailed`
- From: `Running`
- On: `RecoverMemberKickoff`(member_id, phase, error)
- Guards:
  - `recover_failed_phase`
  - `recover_failed_has_error`
- To: `Running`

### `RecoverMemberKickoffCancelled`
- From: `Running`
- On: `RecoverMemberKickoff`(member_id, phase, error)
- Guards:
  - `recover_cancelled_phase`
  - `recover_cancelled_without_error`
- To: `Running`

### `ReconcileRunning`
- From: `Running`
- On: `Reconcile`(desired, retire_stale)
- To: `Running`

### `ReconcileStopped`
- From: `Stopped`
- On: `Reconcile`(desired, retire_stale)
- To: `Stopped`

### `ReconcileCompleted`
- From: `Completed`
- On: `Reconcile`(desired, retire_stale)
- To: `Completed`

### `ReconcileDestroyed`
- From: `Destroyed`
- On: `Reconcile`(desired, retire_stale)
- To: `Destroyed`

### `ObserveRuntimeReady`
- From: `Running`
- On: `ObserveRuntimeReady`(agent_runtime_id, fence_token)
- Guards:
  - `current_binding_matches`
  - `fence_token_matches`
- To: `Running`

### `StartupMarkReadyRunning`
- From: `Running`
- On: `StartupMarkReady`(agent_runtime_id, fence_token)
- Guards:
  - `current_binding_matches`
  - `fence_token_matches`
- To: `Running`

### `StartupMarkReadyStopped`
- From: `Stopped`
- On: `StartupMarkReady`(agent_runtime_id, fence_token)
- Guards:
  - `current_binding_matches`
  - `fence_token_matches`
- To: `Stopped`

### `StartupMarkReadyCompleted`
- From: `Completed`
- On: `StartupMarkReady`(agent_runtime_id, fence_token)
- Guards:
  - `current_binding_matches`
  - `fence_token_matches`
- To: `Completed`

### `KickoffMarkPendingRunning`
- From: `Running`
- On: `KickoffMarkPending`(member_id)
- Guards:
  - `kickoff_not_started`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Running`

### `KickoffMarkPendingStopped`
- From: `Stopped`
- On: `KickoffMarkPending`(member_id)
- Guards:
  - `kickoff_not_started`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Stopped`

### `KickoffMarkPendingCompleted`
- From: `Completed`
- On: `KickoffMarkPending`(member_id)
- Guards:
  - `kickoff_not_started`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Completed`

### `KickoffMarkStartingRunning`
- From: `Running`
- On: `KickoffMarkStarting`(member_id)
- Guards:
  - `kickoff_pending`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Running`

### `KickoffMarkStartingStopped`
- From: `Stopped`
- On: `KickoffMarkStarting`(member_id)
- Guards:
  - `kickoff_pending`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Stopped`

### `KickoffMarkStartingCompleted`
- From: `Completed`
- On: `KickoffMarkStarting`(member_id)
- Guards:
  - `kickoff_pending`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Completed`

### `KickoffResolveStartedRunning`
- From: `Running`
- On: `KickoffResolveStarted`(member_id)
- Guards:
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Running`

### `KickoffResolveStartedStopped`
- From: `Stopped`
- On: `KickoffResolveStarted`(member_id)
- Guards:
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Stopped`

### `KickoffResolveStartedCompleted`
- From: `Completed`
- On: `KickoffResolveStarted`(member_id)
- Guards:
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Completed`

### `KickoffResolveCallbackPendingRunning`
- From: `Running`
- On: `KickoffResolveCallbackPending`(member_id)
- Guards:
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Running`

### `KickoffResolveCallbackPendingStopped`
- From: `Stopped`
- On: `KickoffResolveCallbackPending`(member_id)
- Guards:
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Stopped`

### `KickoffResolveCallbackPendingCompleted`
- From: `Completed`
- On: `KickoffResolveCallbackPending`(member_id)
- Guards:
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Completed`

### `KickoffResolveFailedFromStartingRunning`
- From: `Running`
- On: `KickoffResolveFailed`(member_id, error)
- Guards:
  - `kickoff_active_failed`
- Emits: `PersistKickoffFailureUpdate`, `EmitKickoffLifecycleNotice`
- To: `Running`

### `KickoffResolveFailedFromStartingStopped`
- From: `Stopped`
- On: `KickoffResolveFailed`(member_id, error)
- Guards:
  - `kickoff_active_failed`
- Emits: `PersistKickoffFailureUpdate`, `EmitKickoffLifecycleNotice`
- To: `Stopped`

### `KickoffResolveFailedFromStartingCompleted`
- From: `Completed`
- On: `KickoffResolveFailed`(member_id, error)
- Guards:
  - `kickoff_active_failed`
- Emits: `PersistKickoffFailureUpdate`, `EmitKickoffLifecycleNotice`
- To: `Completed`

### `KickoffCancelRequestedRunning`
- From: `Running`
- On: `KickoffCancelRequested`(member_id)
- Guards:
  - `kickoff_cancellable`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Running`

### `KickoffCancelRequestedStopped`
- From: `Stopped`
- On: `KickoffCancelRequested`(member_id)
- Guards:
  - `kickoff_cancellable`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Stopped`

### `KickoffCancelRequestedCompleted`
- From: `Completed`
- On: `KickoffCancelRequested`(member_id)
- Guards:
  - `kickoff_cancellable`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Completed`

### `KickoffClearRunning`
- From: `Running`
- On: `KickoffClear`(member_id)
- To: `Running`

### `KickoffClearStopped`
- From: `Stopped`
- On: `KickoffClear`(member_id)
- To: `Stopped`

### `KickoffClearCompleted`
- From: `Completed`
- On: `KickoffClear`(member_id)
- To: `Completed`

### `KickoffQuiescedInFlightRunning`
- From: `Running`
- On: `KickoffQuiesced`(member_id)
- Guards:
  - `kickoff_in_flight`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Running`

### `KickoffQuiescedInFlightStopped`
- From: `Stopped`
- On: `KickoffQuiesced`(member_id)
- Guards:
  - `kickoff_in_flight`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Stopped`

### `KickoffQuiescedInFlightCompleted`
- From: `Completed`
- On: `KickoffQuiesced`(member_id)
- Guards:
  - `kickoff_in_flight`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Completed`

### `KickoffQuiescedIdleRunning`
- From: `Running`
- On: `KickoffQuiesced`(member_id)
- Guards:
  - `kickoff_not_in_flight`
- To: `Running`

### `KickoffQuiescedIdleStopped`
- From: `Stopped`
- On: `KickoffQuiesced`(member_id)
- Guards:
  - `kickoff_not_in_flight`
- To: `Stopped`

### `KickoffQuiescedIdleCompleted`
- From: `Completed`
- On: `KickoffQuiesced`(member_id)
- Guards:
  - `kickoff_not_in_flight`
- To: `Completed`

### `KickoffQuiescedDestroyed`
- From: `Destroyed`
- On: `KickoffQuiesced`(member_id)
- To: `Destroyed`

### `KickoffResolveStartedLateArrivalRunning`
- From: `Running`
- On: `KickoffResolveStarted`(member_id)
- Guards:
  - `kickoff_not_starting`
- To: `Running`

### `KickoffResolveStartedLateArrivalStopped`
- From: `Stopped`
- On: `KickoffResolveStarted`(member_id)
- Guards:
  - `kickoff_not_starting`
- To: `Stopped`

### `KickoffResolveStartedLateArrivalCompleted`
- From: `Completed`
- On: `KickoffResolveStarted`(member_id)
- Guards:
  - `kickoff_not_starting`
- To: `Completed`

### `KickoffResolveStartedDestroyed`
- From: `Destroyed`
- On: `KickoffResolveStarted`(member_id)
- To: `Destroyed`

### `KickoffResolveCallbackPendingLateArrivalRunning`
- From: `Running`
- On: `KickoffResolveCallbackPending`(member_id)
- Guards:
  - `kickoff_not_starting`
- To: `Running`

### `KickoffResolveCallbackPendingLateArrivalStopped`
- From: `Stopped`
- On: `KickoffResolveCallbackPending`(member_id)
- Guards:
  - `kickoff_not_starting`
- To: `Stopped`

### `KickoffResolveCallbackPendingLateArrivalCompleted`
- From: `Completed`
- On: `KickoffResolveCallbackPending`(member_id)
- Guards:
  - `kickoff_not_starting`
- To: `Completed`

### `KickoffResolveCallbackPendingDestroyed`
- From: `Destroyed`
- On: `KickoffResolveCallbackPending`(member_id)
- To: `Destroyed`

### `KickoffResolveFailedLateArrivalRunning`
- From: `Running`
- On: `KickoffResolveFailed`(member_id, error)
- Guards:
  - `kickoff_not_active`
- To: `Running`

### `KickoffResolveFailedLateArrivalStopped`
- From: `Stopped`
- On: `KickoffResolveFailed`(member_id, error)
- Guards:
  - `kickoff_not_active`
- To: `Stopped`

### `KickoffResolveFailedLateArrivalCompleted`
- From: `Completed`
- On: `KickoffResolveFailed`(member_id, error)
- Guards:
  - `kickoff_not_active`
- To: `Completed`

### `KickoffResolveFailedDestroyed`
- From: `Destroyed`
- On: `KickoffResolveFailed`(member_id, error)
- To: `Destroyed`

### `KickoffCancelRequestedLateArrivalRunning`
- From: `Running`
- On: `KickoffCancelRequested`(member_id)
- Guards:
  - `kickoff_not_active`
- To: `Running`

### `KickoffCancelRequestedLateArrivalStopped`
- From: `Stopped`
- On: `KickoffCancelRequested`(member_id)
- Guards:
  - `kickoff_not_active`
- To: `Stopped`

### `KickoffCancelRequestedLateArrivalCompleted`
- From: `Completed`
- On: `KickoffCancelRequested`(member_id)
- Guards:
  - `kickoff_not_active`
- To: `Completed`

### `KickoffCancelRequestedDestroyed`
- From: `Destroyed`
- On: `KickoffCancelRequested`(member_id)
- To: `Destroyed`

### `ProbeMemberAdmissionDuplicateRunning`
- From: `Running`
- On: `ProbeMemberAdmission`(agent_identity)
- Guards:
  - `identity_already_bound_or_pending`
- Emits: `MemberAdmissionProbed`
- To: `Running`

### `ProbeMemberAdmissionDuplicateStopped`
- From: `Stopped`
- On: `ProbeMemberAdmission`(agent_identity)
- Guards:
  - `identity_already_bound_or_pending`
- Emits: `MemberAdmissionProbed`
- To: `Stopped`

### `ProbeMemberAdmissionDuplicateCompleted`
- From: `Completed`
- On: `ProbeMemberAdmission`(agent_identity)
- Guards:
  - `identity_already_bound_or_pending`
- Emits: `MemberAdmissionProbed`
- To: `Completed`

### `ProbeMemberAdmissionDuplicateDestroyed`
- From: `Destroyed`
- On: `ProbeMemberAdmission`(agent_identity)
- Guards:
  - `identity_already_bound_or_pending`
- Emits: `MemberAdmissionProbed`
- To: `Destroyed`

### `ProbeMemberAdmissionAdmittedRunning`
- From: `Running`
- On: `ProbeMemberAdmission`(agent_identity)
- Guards:
  - `identity_not_bound_and_not_pending`
- Emits: `MemberAdmissionProbed`
- To: `Running`

### `ProbeMemberAdmissionAdmittedStopped`
- From: `Stopped`
- On: `ProbeMemberAdmission`(agent_identity)
- Guards:
  - `identity_not_bound_and_not_pending`
- Emits: `MemberAdmissionProbed`
- To: `Stopped`

### `ProbeMemberAdmissionAdmittedCompleted`
- From: `Completed`
- On: `ProbeMemberAdmission`(agent_identity)
- Guards:
  - `identity_not_bound_and_not_pending`
- Emits: `MemberAdmissionProbed`
- To: `Completed`

### `ProbeMemberAdmissionAdmittedDestroyed`
- From: `Destroyed`
- On: `ProbeMemberAdmission`(agent_identity)
- Guards:
  - `identity_not_bound_and_not_pending`
- Emits: `MemberAdmissionProbed`
- To: `Destroyed`

### `ComputeRespawnGenerationRunning`
- From: `Running`
- On: `ComputeRespawnGeneration`(agent_identity)
- Emits: `RespawnGenerationComputed`
- To: `Running`

### `ComputeRespawnGenerationStopped`
- From: `Stopped`
- On: `ComputeRespawnGeneration`(agent_identity)
- Emits: `RespawnGenerationComputed`
- To: `Stopped`

### `ComputeRespawnGenerationCompleted`
- From: `Completed`
- On: `ComputeRespawnGeneration`(agent_identity)
- Emits: `RespawnGenerationComputed`
- To: `Completed`

### `ComputeRespawnGenerationDestroyed`
- From: `Destroyed`
- On: `ComputeRespawnGeneration`(agent_identity)
- Emits: `RespawnGenerationComputed`
- To: `Destroyed`

### `ClassifyStepOutputFaultRetryRunning`
- From: `Running`
- On: `ClassifyStepOutputFault`(run_id, step_id, target_retry_key, fault, attempt, max_retries)
- Guards:
  - `attempts_remaining`
- Emits: `StepOutputFaultClassified`
- To: `Running`

### `ClassifyStepOutputFaultRetryStopped`
- From: `Stopped`
- On: `ClassifyStepOutputFault`(run_id, step_id, target_retry_key, fault, attempt, max_retries)
- Guards:
  - `attempts_remaining`
- Emits: `StepOutputFaultClassified`
- To: `Stopped`

### `ClassifyStepOutputFaultRetryCompleted`
- From: `Completed`
- On: `ClassifyStepOutputFault`(run_id, step_id, target_retry_key, fault, attempt, max_retries)
- Guards:
  - `attempts_remaining`
- Emits: `StepOutputFaultClassified`
- To: `Completed`

### `ClassifyStepOutputFaultRetryDestroyed`
- From: `Destroyed`
- On: `ClassifyStepOutputFault`(run_id, step_id, target_retry_key, fault, attempt, max_retries)
- Guards:
  - `attempts_remaining`
- Emits: `StepOutputFaultClassified`
- To: `Destroyed`

### `ClassifyStepOutputFaultTerminalRunning`
- From: `Running`
- On: `ClassifyStepOutputFault`(run_id, step_id, target_retry_key, fault, attempt, max_retries)
- Guards:
  - `attempts_exhausted`
- Emits: `StepOutputFaultClassified`
- To: `Running`

### `ClassifyStepOutputFaultTerminalStopped`
- From: `Stopped`
- On: `ClassifyStepOutputFault`(run_id, step_id, target_retry_key, fault, attempt, max_retries)
- Guards:
  - `attempts_exhausted`
- Emits: `StepOutputFaultClassified`
- To: `Stopped`

### `ClassifyStepOutputFaultTerminalCompleted`
- From: `Completed`
- On: `ClassifyStepOutputFault`(run_id, step_id, target_retry_key, fault, attempt, max_retries)
- Guards:
  - `attempts_exhausted`
- Emits: `StepOutputFaultClassified`
- To: `Completed`

### `ClassifyStepOutputFaultTerminalDestroyed`
- From: `Destroyed`
- On: `ClassifyStepOutputFault`(run_id, step_id, target_retry_key, fault, attempt, max_retries)
- Guards:
  - `attempts_exhausted`
- Emits: `StepOutputFaultClassified`
- To: `Destroyed`

### `EscalateToSupervisorTargetFoundRunning`
- From: `Running`
- On: `EscalateToSupervisor`(run_id, step_id, supervisor_identity, turn_timeout_ms)
- Emits: `SupervisorEscalationRequested`
- To: `Running`

### `EscalateToSupervisorTargetFoundStopped`
- From: `Stopped`
- On: `EscalateToSupervisor`(run_id, step_id, supervisor_identity, turn_timeout_ms)
- Emits: `SupervisorEscalationRequested`
- To: `Stopped`

### `EscalateToSupervisorTargetFoundCompleted`
- From: `Completed`
- On: `EscalateToSupervisor`(run_id, step_id, supervisor_identity, turn_timeout_ms)
- Emits: `SupervisorEscalationRequested`
- To: `Completed`

### `EscalateToSupervisorTargetFoundDestroyed`
- From: `Destroyed`
- On: `EscalateToSupervisor`(run_id, step_id, supervisor_identity, turn_timeout_ms)
- Emits: `SupervisorEscalationRequested`
- To: `Destroyed`

### `EscalateToSupervisorTargetMissingRunning`
- From: `Running`
- On: `EscalateToSupervisorNoEligibleTarget`(run_id, step_id)
- Emits: `SupervisorEscalationFailed`
- To: `Running`

### `EscalateToSupervisorTargetMissingStopped`
- From: `Stopped`
- On: `EscalateToSupervisorNoEligibleTarget`(run_id, step_id)
- Emits: `SupervisorEscalationFailed`
- To: `Stopped`

### `EscalateToSupervisorTargetMissingCompleted`
- From: `Completed`
- On: `EscalateToSupervisorNoEligibleTarget`(run_id, step_id)
- Emits: `SupervisorEscalationFailed`
- To: `Completed`

### `EscalateToSupervisorTargetMissingDestroyed`
- From: `Destroyed`
- On: `EscalateToSupervisorNoEligibleTarget`(run_id, step_id)
- Emits: `SupervisorEscalationFailed`
- To: `Destroyed`

### `EvaluateTopologyEdgeRuleAllowRunning`
- From: `Running`
- On: `EvaluateTopologyEdge`(from_role, to_role, rule_match)
- Guards:
  - `rule_allows_edge`
- Emits: `TopologyEdgeVerdictResolved`
- To: `Running`

### `EvaluateTopologyEdgeRuleAllowStopped`
- From: `Stopped`
- On: `EvaluateTopologyEdge`(from_role, to_role, rule_match)
- Guards:
  - `rule_allows_edge`
- Emits: `TopologyEdgeVerdictResolved`
- To: `Stopped`

### `EvaluateTopologyEdgeRuleAllowCompleted`
- From: `Completed`
- On: `EvaluateTopologyEdge`(from_role, to_role, rule_match)
- Guards:
  - `rule_allows_edge`
- Emits: `TopologyEdgeVerdictResolved`
- To: `Completed`

### `EvaluateTopologyEdgeRuleAllowDestroyed`
- From: `Destroyed`
- On: `EvaluateTopologyEdge`(from_role, to_role, rule_match)
- Guards:
  - `rule_allows_edge`
- Emits: `TopologyEdgeVerdictResolved`
- To: `Destroyed`

### `EvaluateTopologyEdgeRuleDenyRunning`
- From: `Running`
- On: `EvaluateTopologyEdge`(from_role, to_role, rule_match)
- Guards:
  - `rule_denies_edge`
- Emits: `TopologyEdgeVerdictResolved`
- To: `Running`

### `EvaluateTopologyEdgeRuleDenyStopped`
- From: `Stopped`
- On: `EvaluateTopologyEdge`(from_role, to_role, rule_match)
- Guards:
  - `rule_denies_edge`
- Emits: `TopologyEdgeVerdictResolved`
- To: `Stopped`

### `EvaluateTopologyEdgeRuleDenyCompleted`
- From: `Completed`
- On: `EvaluateTopologyEdge`(from_role, to_role, rule_match)
- Guards:
  - `rule_denies_edge`
- Emits: `TopologyEdgeVerdictResolved`
- To: `Completed`

### `EvaluateTopologyEdgeRuleDenyDestroyed`
- From: `Destroyed`
- On: `EvaluateTopologyEdge`(from_role, to_role, rule_match)
- Guards:
  - `rule_denies_edge`
- Emits: `TopologyEdgeVerdictResolved`
- To: `Destroyed`

### `EvaluateTopologyEdgeDefaultAllowRunning`
- From: `Running`
- On: `EvaluateTopologyEdge`(from_role, to_role, rule_match)
- Guards:
  - `no_rule_and_default_allow`
- Emits: `TopologyEdgeVerdictResolved`
- To: `Running`

### `EvaluateTopologyEdgeDefaultAllowStopped`
- From: `Stopped`
- On: `EvaluateTopologyEdge`(from_role, to_role, rule_match)
- Guards:
  - `no_rule_and_default_allow`
- Emits: `TopologyEdgeVerdictResolved`
- To: `Stopped`

### `EvaluateTopologyEdgeDefaultAllowCompleted`
- From: `Completed`
- On: `EvaluateTopologyEdge`(from_role, to_role, rule_match)
- Guards:
  - `no_rule_and_default_allow`
- Emits: `TopologyEdgeVerdictResolved`
- To: `Completed`

### `EvaluateTopologyEdgeDefaultAllowDestroyed`
- From: `Destroyed`
- On: `EvaluateTopologyEdge`(from_role, to_role, rule_match)
- Guards:
  - `no_rule_and_default_allow`
- Emits: `TopologyEdgeVerdictResolved`
- To: `Destroyed`

### `EvaluateTopologyEdgeDefaultDenyRunning`
- From: `Running`
- On: `EvaluateTopologyEdge`(from_role, to_role, rule_match)
- Guards:
  - `no_rule_and_default_deny`
- Emits: `TopologyEdgeVerdictResolved`
- To: `Running`

### `EvaluateTopologyEdgeDefaultDenyStopped`
- From: `Stopped`
- On: `EvaluateTopologyEdge`(from_role, to_role, rule_match)
- Guards:
  - `no_rule_and_default_deny`
- Emits: `TopologyEdgeVerdictResolved`
- To: `Stopped`

### `EvaluateTopologyEdgeDefaultDenyCompleted`
- From: `Completed`
- On: `EvaluateTopologyEdge`(from_role, to_role, rule_match)
- Guards:
  - `no_rule_and_default_deny`
- Emits: `TopologyEdgeVerdictResolved`
- To: `Completed`

### `EvaluateTopologyEdgeDefaultDenyDestroyed`
- From: `Destroyed`
- On: `EvaluateTopologyEdge`(from_role, to_role, rule_match)
- Guards:
  - `no_rule_and_default_deny`
- Emits: `TopologyEdgeVerdictResolved`
- To: `Destroyed`

### `SetExternalMemberRebindCapabilityRunning`
- From: `Running`
- On: `SetExternalMemberRebindCapability`(agent_identity, capability)
- To: `Running`

### `SetExternalMemberRebindCapabilityStopped`
- From: `Stopped`
- On: `SetExternalMemberRebindCapability`(agent_identity, capability)
- To: `Stopped`

### `SetExternalMemberRebindCapabilityCompleted`
- From: `Completed`
- On: `SetExternalMemberRebindCapability`(agent_identity, capability)
- To: `Completed`

### `SetExternalMemberRebindCapabilityDestroyed`
- From: `Destroyed`
- On: `SetExternalMemberRebindCapability`(agent_identity, capability)
- To: `Destroyed`

### `SeedOrphanBudgetRunning`
- From: `Running`
- On: `SeedOrphanBudget`(budget)
- To: `Running`

### `SeedOrphanBudgetStopped`
- From: `Stopped`
- On: `SeedOrphanBudget`(budget)
- To: `Stopped`

### `SeedOrphanBudgetCompleted`
- From: `Completed`
- On: `SeedOrphanBudget`(budget)
- To: `Completed`

### `SeedOrphanBudgetDestroyed`
- From: `Destroyed`
- On: `SeedOrphanBudget`(budget)
- To: `Destroyed`

### `ClassifyTurnTimeoutDispositionRetryableRunning`
- From: `Running`
- On: `ClassifyTurnTimeoutDisposition`(timed_out_run_id, retryable)
- Guards:
  - `timeout_is_retryable`
- Emits: `TurnTimeoutDispositionClassified`
- To: `Running`

### `ClassifyTurnTimeoutDispositionRetryableStopped`
- From: `Stopped`
- On: `ClassifyTurnTimeoutDisposition`(timed_out_run_id, retryable)
- Guards:
  - `timeout_is_retryable`
- Emits: `TurnTimeoutDispositionClassified`
- To: `Stopped`

### `ClassifyTurnTimeoutDispositionRetryableCompleted`
- From: `Completed`
- On: `ClassifyTurnTimeoutDisposition`(timed_out_run_id, retryable)
- Guards:
  - `timeout_is_retryable`
- Emits: `TurnTimeoutDispositionClassified`
- To: `Completed`

### `ClassifyTurnTimeoutDispositionRetryableDestroyed`
- From: `Destroyed`
- On: `ClassifyTurnTimeoutDisposition`(timed_out_run_id, retryable)
- Guards:
  - `timeout_is_retryable`
- Emits: `TurnTimeoutDispositionClassified`
- To: `Destroyed`

### `ClassifyTurnTimeoutDispositionDetachedRunning`
- From: `Running`
- On: `ClassifyTurnTimeoutDisposition`(timed_out_run_id, retryable)
- Guards:
  - `non_retryable_with_budget`
- Emits: `TurnTimeoutDispositionClassified`
- To: `Running`

### `ClassifyTurnTimeoutDispositionDetachedStopped`
- From: `Stopped`
- On: `ClassifyTurnTimeoutDisposition`(timed_out_run_id, retryable)
- Guards:
  - `non_retryable_with_budget`
- Emits: `TurnTimeoutDispositionClassified`
- To: `Stopped`

### `ClassifyTurnTimeoutDispositionDetachedCompleted`
- From: `Completed`
- On: `ClassifyTurnTimeoutDisposition`(timed_out_run_id, retryable)
- Guards:
  - `non_retryable_with_budget`
- Emits: `TurnTimeoutDispositionClassified`
- To: `Completed`

### `ClassifyTurnTimeoutDispositionDetachedDestroyed`
- From: `Destroyed`
- On: `ClassifyTurnTimeoutDisposition`(timed_out_run_id, retryable)
- Guards:
  - `non_retryable_with_budget`
- Emits: `TurnTimeoutDispositionClassified`
- To: `Destroyed`

### `ClassifyTurnTimeoutDispositionCanceledRunning`
- From: `Running`
- On: `ClassifyTurnTimeoutDisposition`(timed_out_run_id, retryable)
- Guards:
  - `non_retryable_without_budget`
- Emits: `TurnTimeoutDispositionClassified`
- To: `Running`

### `ClassifyTurnTimeoutDispositionCanceledStopped`
- From: `Stopped`
- On: `ClassifyTurnTimeoutDisposition`(timed_out_run_id, retryable)
- Guards:
  - `non_retryable_without_budget`
- Emits: `TurnTimeoutDispositionClassified`
- To: `Stopped`

### `ClassifyTurnTimeoutDispositionCanceledCompleted`
- From: `Completed`
- On: `ClassifyTurnTimeoutDisposition`(timed_out_run_id, retryable)
- Guards:
  - `non_retryable_without_budget`
- Emits: `TurnTimeoutDispositionClassified`
- To: `Completed`

### `ClassifyTurnTimeoutDispositionCanceledDestroyed`
- From: `Destroyed`
- On: `ClassifyTurnTimeoutDisposition`(timed_out_run_id, retryable)
- Guards:
  - `non_retryable_without_budget`
- Emits: `TurnTimeoutDispositionClassified`
- To: `Destroyed`

### `SubmitWorkRunningExternal`
- From: `Running`
- On: `SubmitWork`(agent_identity, agent_runtime_id, fence_token, work_id, origin)
- Guards:
  - `active_members_present`
  - `identity_binding_matches`
  - `current_binding_matches`
  - `fence_token_matches`
  - `generation_binding_present`
  - `session_binding_present`
  - `member_not_retiring`
  - `external_origin`
  - `runtime_externally_addressable`
- Emits: `RequestRuntimeIngress`
- To: `Running`

### `SubmitWorkRunningExternalPeerOnly`
- From: `Running`
- On: `SubmitWork`(agent_identity, agent_runtime_id, fence_token, work_id, origin)
- Guards:
  - `active_members_present`
  - `identity_binding_matches`
  - `current_binding_matches`
  - `fence_token_matches`
  - `generation_binding_present`
  - `session_binding_absent`
  - `member_not_retiring`
  - `external_origin`
  - `runtime_externally_addressable`
  - `member_peer_registered`
- Emits: `RequestPeerRuntimeIngress`
- To: `Running`

### `SubmitWorkRunningInternal`
- From: `Running`
- On: `SubmitWork`(agent_identity, agent_runtime_id, fence_token, work_id, origin)
- Guards:
  - `active_members_present`
  - `identity_binding_matches`
  - `current_binding_matches`
  - `fence_token_matches`
  - `generation_binding_present`
  - `session_binding_present`
  - `member_not_retiring`
  - `internal_origin`
- Emits: `RequestRuntimeIngress`
- To: `Running`

### `SubmitWorkRunningInternalPeerOnly`
- From: `Running`
- On: `SubmitWork`(agent_identity, agent_runtime_id, fence_token, work_id, origin)
- Guards:
  - `active_members_present`
  - `identity_binding_matches`
  - `current_binding_matches`
  - `fence_token_matches`
  - `generation_binding_present`
  - `session_binding_absent`
  - `member_not_retiring`
  - `internal_origin`
  - `member_peer_registered`
- Emits: `RequestPeerRuntimeIngress`
- To: `Running`

### `ResolveSubmitWorkRejectionStopped`
- From: `Stopped`
- On: `ResolveSubmitWorkRejection`(agent_identity, agent_runtime_id, fence_token, origin)
- Emits: `SubmitWorkRejected`
- To: `Stopped`

### `ResolveSubmitWorkRejectionCompleted`
- From: `Completed`
- On: `ResolveSubmitWorkRejection`(agent_identity, agent_runtime_id, fence_token, origin)
- Emits: `SubmitWorkRejected`
- To: `Completed`

### `ResolveSubmitWorkRejectionDestroyed`
- From: `Destroyed`
- On: `ResolveSubmitWorkRejection`(agent_identity, agent_runtime_id, fence_token, origin)
- Emits: `SubmitWorkRejected`
- To: `Destroyed`

### `ResolveSubmitWorkRejectionMemberNotFound`
- From: `Running`
- On: `ResolveSubmitWorkRejection`(agent_identity, agent_runtime_id, fence_token, origin)
- Guards:
  - `identity_absent`
- Emits: `SubmitWorkRejected`
- To: `Running`

### `ResolveSubmitWorkRejectionCurrentRuntimeNotLive`
- From: `Running`
- On: `ResolveSubmitWorkRejection`(agent_identity, agent_runtime_id, fence_token, origin)
- Guards:
  - `identity_present`
  - `current_runtime_not_live`
- Emits: `SubmitWorkRejected`
- To: `Running`

### `ResolveSubmitWorkRejectionStaleFenceToken`
- From: `Running`
- On: `ResolveSubmitWorkRejection`(agent_identity, agent_runtime_id, fence_token, origin)
- Guards:
  - `identity_present`
  - `current_runtime_live`
  - `runtime_or_fence_stale`
- Emits: `SubmitWorkRejected`
- To: `Running`

### `ResolveSubmitWorkRejectionRetiringAsMemberNotFound`
- From: `Running`
- On: `ResolveSubmitWorkRejection`(agent_identity, agent_runtime_id, fence_token, origin)
- Guards:
  - `identity_binding_matches`
  - `runtime_live`
  - `fence_token_matches`
  - `member_retiring`
- Emits: `SubmitWorkRejected`
- To: `Running`

### `ResolveSubmitWorkRejectionNotExternallyAddressable`
- From: `Running`
- On: `ResolveSubmitWorkRejection`(agent_identity, agent_runtime_id, fence_token, origin)
- Guards:
  - `identity_binding_matches`
  - `runtime_live`
  - `fence_token_matches`
  - `member_not_retiring`
  - `external_origin`
  - `runtime_not_externally_addressable`
- Emits: `SubmitWorkRejected`
- To: `Running`

### `ResolveSubmitWorkRejectionPeerOnlyNotExternallyAddressable`
- From: `Running`
- On: `ResolveSubmitWorkRejection`(agent_identity, agent_runtime_id, fence_token, origin)
- Guards:
  - `identity_binding_matches`
  - `runtime_live`
  - `fence_token_matches`
  - `member_not_retiring`
  - `internal_origin`
  - `session_binding_absent`
  - `member_peer_absent`
- Emits: `SubmitWorkRejected`
- To: `Running`

### `ResolveRuntimeBindingRefusalRunning`
- From: `Running`
- On: `ResolveRuntimeBindingRefusal`(agent_identity, agent_runtime_id, session_id, refusal_code, reason)
- Guards:
  - `identity_binding_matches`
  - `current_binding_matches`
  - `session_binding_matches`
- Emits: `RuntimeBindingRefusalClassified`
- To: `Running`

### `ResolveRuntimeIngressRefusalRunning`
- From: `Running`
- On: `ResolveRuntimeIngressRefusal`(agent_runtime_id, fence_token, session_id, work_id, origin, refusal_code, reason)
- Guards:
  - `runtime_live`
  - `fence_token_matches`
- Emits: `RuntimeIngressRefusalClassified`
- To: `Running`

### `ResolveRuntimeRetireRefusalRunning`
- From: `Running`
- On: `ResolveRuntimeRetireRefusal`(agent_identity, agent_runtime_id, session_id, refusal_code, reason)
- Guards:
  - `identity_binding_matches`
  - `member_retiring`
- Emits: `RuntimeRetireRefusalClassified`
- To: `Running`

### `ResolveRuntimeRetireRefusalStopped`
- From: `Stopped`
- On: `ResolveRuntimeRetireRefusal`(agent_identity, agent_runtime_id, session_id, refusal_code, reason)
- Guards:
  - `identity_binding_matches`
  - `member_retiring`
- Emits: `RuntimeRetireRefusalClassified`
- To: `Stopped`

### `RetryRuntimeRetireRunning`
- From: `Running`
- On: `RetryRuntimeRetire`(agent_identity, agent_runtime_id)
- Guards:
  - `identity_binding_matches`
  - `member_retiring`
  - `retirement_pending`
- Emits: `RequestRuntimeRetire`
- To: `Running`

### `RetryRuntimeRetireStopped`
- From: `Stopped`
- On: `RetryRuntimeRetire`(agent_identity, agent_runtime_id)
- Guards:
  - `identity_binding_matches`
  - `member_retiring`
  - `retirement_pending`
- Emits: `RequestRuntimeRetire`
- To: `Stopped`

### `RecordRemoteMemberRuntimeRetiredFreshRunning`
- From: `Running`
- On: `RecordRemoteMemberRuntimeRetired`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `remote_member_session_absent`
  - `remote_runtime_not_recorded`
- Emits: `AppendLifecycleJournal`
- To: `Running`

### `RecordRemoteMemberRuntimeRetiredFreshStopped`
- From: `Stopped`
- On: `RecordRemoteMemberRuntimeRetired`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `remote_member_session_absent`
  - `remote_runtime_not_recorded`
- Emits: `AppendLifecycleJournal`
- To: `Stopped`

### `RecordRemoteMemberRuntimeRetiredAlreadyRecordedRunning`
- From: `Running`
- On: `RecordRemoteMemberRuntimeRetired`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `remote_member_session_absent`
  - `remote_runtime_recorded`
- Emits: `AppendLifecycleJournal`
- To: `Running`

### `RecordRemoteMemberRuntimeRetiredAlreadyRecordedStopped`
- From: `Stopped`
- On: `RecordRemoteMemberRuntimeRetired`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `remote_member_session_absent`
  - `remote_runtime_recorded`
- Emits: `AppendLifecycleJournal`
- To: `Stopped`

### `RecordRemoteMemberSupervisorRevokedFreshRunning`
- From: `Running`
- On: `RecordRemoteMemberSupervisorRevoked`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `remote_member_session_absent`
  - `remote_runtime_recorded`
  - `remote_supervisor_not_recorded`
- Emits: `AppendLifecycleJournal`
- To: `Running`

### `RecordRemoteMemberSupervisorRevokedFreshStopped`
- From: `Stopped`
- On: `RecordRemoteMemberSupervisorRevoked`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `remote_member_session_absent`
  - `remote_runtime_recorded`
  - `remote_supervisor_not_recorded`
- Emits: `AppendLifecycleJournal`
- To: `Stopped`

### `RecordRemoteMemberSupervisorRevokedAlreadyRecordedRunning`
- From: `Running`
- On: `RecordRemoteMemberSupervisorRevoked`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `remote_member_session_absent`
  - `remote_runtime_recorded`
  - `remote_supervisor_recorded`
- Emits: `AppendLifecycleJournal`
- To: `Running`

### `RecordRemoteMemberSupervisorRevokedAlreadyRecordedStopped`
- From: `Stopped`
- On: `RecordRemoteMemberSupervisorRevoked`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `remote_member_session_absent`
  - `remote_runtime_recorded`
  - `remote_supervisor_recorded`
- Emits: `AppendLifecycleJournal`
- To: `Stopped`

### `AdmitDestroyMemberRetireLiveRunning`
- From: `Running`
- On: `AdmitDestroyMemberRetire`(mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `destroy_admitted`
  - `identity_binding_matches`
  - `current_binding_matches`
  - `fence_token_matches`
  - `generation_matches`
  - `member_not_retiring`
  - `session_ingress_detach_absent`
  - `retirement_session_absent`
  - `retire_refusal_absent`
  - `session_present`
  - `session_binding_matches`
- Emits: `AppendLifecycleJournal`, `RequestRuntimeRetire`, `RequestSessionIngressDetachForMobDestroy`, `RequestKickoffQuiesce`
- To: `Running`

### `AdmitDestroyMemberRetireLiveStopped`
- From: `Stopped`
- On: `AdmitDestroyMemberRetire`(mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `destroy_admitted`
  - `identity_binding_matches`
  - `current_binding_matches`
  - `fence_token_matches`
  - `generation_matches`
  - `member_not_retiring`
  - `session_ingress_detach_absent`
  - `retirement_session_absent`
  - `retire_refusal_absent`
  - `session_present`
  - `session_binding_matches`
- Emits: `AppendLifecycleJournal`, `RequestRuntimeRetire`, `RequestSessionIngressDetachForMobDestroy`, `RequestKickoffQuiesce`
- To: `Stopped`

### `AdmitDestroyMemberRetirePeerOnlyRunning`
- From: `Running`
- On: `AdmitDestroyMemberRetire`(mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `destroy_admitted`
  - `identity_binding_matches`
  - `current_binding_matches`
  - `fence_token_matches`
  - `generation_matches`
  - `member_not_retiring`
  - `session_ingress_detach_absent`
  - `retirement_session_absent`
  - `retire_refusal_absent`
  - `session_absent`
  - `session_binding_absent`
- Emits: `AppendLifecycleJournal`, `RequestKickoffQuiesce`
- To: `Running`

### `AdmitDestroyMemberRetirePeerOnlyStopped`
- From: `Stopped`
- On: `AdmitDestroyMemberRetire`(mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `destroy_admitted`
  - `identity_binding_matches`
  - `current_binding_matches`
  - `fence_token_matches`
  - `generation_matches`
  - `member_not_retiring`
  - `session_ingress_detach_absent`
  - `retirement_session_absent`
  - `retire_refusal_absent`
  - `session_absent`
  - `session_binding_absent`
- Emits: `AppendLifecycleJournal`, `RequestKickoffQuiesce`
- To: `Stopped`

### `AdmitDestroyMemberRetireAlreadyRetiringSessionRunning`
- From: `Running`
- On: `AdmitDestroyMemberRetire`(mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `destroy_admitted`
  - `identity_binding_matches`
  - `member_retiring`
  - `fence_token_matches`
  - `generation_matches`
  - `session_present`
  - `session_binding_matches`
  - `retirement_session_matches`
- Emits: `RequestSessionIngressDetachForMobDestroy`, `RequestKickoffQuiesce`
- To: `Running`

### `AdmitDestroyMemberRetireAlreadyRetiringSessionStopped`
- From: `Stopped`
- On: `AdmitDestroyMemberRetire`(mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `destroy_admitted`
  - `identity_binding_matches`
  - `member_retiring`
  - `fence_token_matches`
  - `generation_matches`
  - `session_present`
  - `session_binding_matches`
  - `retirement_session_matches`
- Emits: `RequestSessionIngressDetachForMobDestroy`, `RequestKickoffQuiesce`
- To: `Stopped`

### `AdmitDestroyMemberRetireAlreadyRetiringReleasedSessionRunning`
- From: `Running`
- On: `AdmitDestroyMemberRetire`(mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `destroy_admitted`
  - `identity_binding_matches`
  - `member_retiring`
  - `fence_token_matches`
  - `generation_matches`
  - `session_present`
  - `session_binding_released`
  - `retirement_session_matches`
- Emits: `RequestSessionIngressDetachForMobDestroy`, `RequestKickoffQuiesce`
- To: `Running`

### `AdmitDestroyMemberRetireAlreadyRetiringReleasedSessionStopped`
- From: `Stopped`
- On: `AdmitDestroyMemberRetire`(mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `destroy_admitted`
  - `identity_binding_matches`
  - `member_retiring`
  - `fence_token_matches`
  - `generation_matches`
  - `session_present`
  - `session_binding_released`
  - `retirement_session_matches`
- Emits: `RequestSessionIngressDetachForMobDestroy`, `RequestKickoffQuiesce`
- To: `Stopped`

### `AdmitDestroyMemberRetireAlreadyRetiringPeerOnlyRunning`
- From: `Running`
- On: `AdmitDestroyMemberRetire`(mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `destroy_admitted`
  - `identity_binding_matches`
  - `member_retiring`
  - `fence_token_matches`
  - `generation_matches`
  - `session_absent`
  - `session_binding_absent`
  - `session_ingress_detach_absent`
  - `retirement_session_absent`
  - `retire_refusal_absent`
- Emits: `RequestKickoffQuiesce`
- To: `Running`

### `AdmitDestroyMemberRetireAlreadyRetiringPeerOnlyStopped`
- From: `Stopped`
- On: `AdmitDestroyMemberRetire`(mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `destroy_admitted`
  - `identity_binding_matches`
  - `member_retiring`
  - `fence_token_matches`
  - `generation_matches`
  - `session_absent`
  - `session_binding_absent`
  - `session_ingress_detach_absent`
  - `retirement_session_absent`
  - `retire_refusal_absent`
- Emits: `RequestKickoffQuiesce`
- To: `Stopped`

### `AdmitDestroyMemberRetireAlreadyArchivedRunning`
- From: `Running`
- On: `AdmitDestroyMemberRetire`(mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `destroy_admitted`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `runtime_not_live`
  - `member_not_retiring`
  - `session_binding_cleared`
  - `retry_observes_cleared_session`
  - `session_ingress_detach_closed`
  - `runtime_retirement_closed`
  - `runtime_retire_refusal_closed`
- To: `Running`

### `AdmitDestroyMemberRetireAlreadyArchivedStopped`
- From: `Stopped`
- On: `AdmitDestroyMemberRetire`(mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `destroy_admitted`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `runtime_not_live`
  - `member_not_retiring`
  - `session_binding_cleared`
  - `retry_observes_cleared_session`
  - `session_ingress_detach_closed`
  - `runtime_retirement_closed`
  - `runtime_retire_refusal_closed`
- To: `Stopped`

### `ObserveRuntimeRetired`
- From: `Running`
- On: `ObserveRuntimeRetired`(agent_runtime_id, fence_token)
- Guards:
  - `current_binding_matches`
  - `fence_token_matches`
- Emits: `EmitMemberLifecycleNotice`
- To: `Running`

### `ObserveRuntimeRetiredStopped`
- From: `Stopped`
- On: `ObserveRuntimeRetired`(agent_runtime_id, fence_token)
- Guards:
  - `current_binding_matches`
  - `fence_token_matches`
- Emits: `EmitMemberLifecycleNotice`
- To: `Stopped`

### `ObserveRemoteMemberRetirementArchivedAndSupervisorRevokedRunning`
- From: `Running`
- On: `ObserveRemoteMemberRetirementArchivedAndSupervisorRevoked`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `fence_token_matches`
  - `generation_matches`
  - `member_retiring`
  - `remote_runtime_retired`
  - `remote_supervisor_revoked`
  - `remote_member_session_absent`
  - `remote_retirement_session_absent`
  - `kickoff_quiesced`
- Emits: `AppendLifecycleJournal`, `EmitMemberLifecycleNotice`
- To: `Running`

### `ObserveRemoteMemberRetirementArchivedAndSupervisorRevokedStopped`
- From: `Stopped`
- On: `ObserveRemoteMemberRetirementArchivedAndSupervisorRevoked`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `fence_token_matches`
  - `generation_matches`
  - `member_retiring`
  - `remote_runtime_retired`
  - `remote_supervisor_revoked`
  - `remote_member_session_absent`
  - `remote_retirement_session_absent`
  - `kickoff_quiesced`
- Emits: `AppendLifecycleJournal`, `EmitMemberLifecycleNotice`
- To: `Stopped`

### `ObserveMemberRetirementArchivedLive`
- From: `Running`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `runtime_live`
  - `fence_token_matches`
  - `generation_matches`
  - `member_retiring`
  - `retirement_session_matches`
  - `session_present`
  - `kickoff_quiesced`
- Emits: `AppendLifecycleJournal`, `EmitMemberLifecycleNotice`
- To: `Running`

### `ObserveMemberRetirementArchivedLiveStopped`
- From: `Stopped`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `runtime_live`
  - `fence_token_matches`
  - `generation_matches`
  - `member_retiring`
  - `retirement_session_matches`
  - `session_present`
  - `kickoff_quiesced`
- Emits: `AppendLifecycleJournal`, `EmitMemberLifecycleNotice`
- To: `Stopped`

### `ObserveMemberRetirementArchivedRetired`
- From: `Running`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `runtime_not_live`
  - `member_retiring`
  - `generation_matches`
  - `retirement_session_matches`
  - `session_present`
  - `kickoff_quiesced`
- Emits: `AppendLifecycleJournal`
- To: `Running`

### `ObserveMemberRetirementArchivedRetiredStopped`
- From: `Stopped`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `runtime_not_live`
  - `member_retiring`
  - `generation_matches`
  - `retirement_session_matches`
  - `session_present`
  - `kickoff_quiesced`
- Emits: `AppendLifecycleJournal`
- To: `Stopped`

### `ObserveMemberRetirementArchivedStaleRuntime`
- From: `Running`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `session_ingress_detach_closed`
  - `identity_remapped`
  - `runtime_not_live`
  - `member_retiring`
  - `retirement_session_matches`
  - `session_present`
- Emits: `AppendLifecycleJournal`
- To: `Running`

### `ObserveMemberRetirementArchivedStaleRuntimeStopped`
- From: `Stopped`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `session_ingress_detach_closed`
  - `identity_remapped`
  - `runtime_not_live`
  - `member_retiring`
  - `retirement_session_matches`
  - `session_present`
- Emits: `AppendLifecycleJournal`
- To: `Stopped`

### `ObserveMemberRetirementArchivedAlreadyCleared`
- From: `Running`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `runtime_not_live`
  - `member_not_retiring`
- Emits: `AppendLifecycleJournal`
- To: `Running`

### `ObserveMemberRetirementArchivedAlreadyClearedStopped`
- From: `Stopped`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `runtime_not_live`
  - `member_not_retiring`
- Emits: `AppendLifecycleJournal`
- To: `Stopped`

### `ObserveMemberRetirementArchivedStaleRuntimeAlreadyCleared`
- From: `Running`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `session_ingress_detach_closed`
  - `identity_remapped`
  - `runtime_not_live`
  - `member_not_retiring`
- Emits: `AppendLifecycleJournal`
- To: `Running`

### `ObserveMemberRetirementArchivedStaleRuntimeAlreadyClearedStopped`
- From: `Stopped`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `session_ingress_detach_closed`
  - `identity_remapped`
  - `runtime_not_live`
  - `member_not_retiring`
- Emits: `AppendLifecycleJournal`
- To: `Stopped`

### `ObserveDestroyMemberRetirementArchivedLiveRunning`
- From: `Running`
- On: `ObserveDestroyMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `session_ingress_detach_closed`
  - `destroy_admitted`
  - `identity_binding_matches`
  - `generation_matches`
  - `session_binding_matches`
  - `session_present`
  - `runtime_live`
  - `fence_token_matches`
  - `member_retiring`
  - `retirement_session_matches`
  - `kickoff_quiesced`
- Emits: `AppendLifecycleJournal`, `EmitMemberLifecycleNotice`, `MemberSessionBindingChanged`
- To: `Running`

### `ObserveDestroyMemberRetirementArchivedLiveStopped`
- From: `Stopped`
- On: `ObserveDestroyMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `session_ingress_detach_closed`
  - `destroy_admitted`
  - `identity_binding_matches`
  - `generation_matches`
  - `session_binding_matches`
  - `session_present`
  - `runtime_live`
  - `fence_token_matches`
  - `member_retiring`
  - `retirement_session_matches`
  - `kickoff_quiesced`
- Emits: `AppendLifecycleJournal`, `EmitMemberLifecycleNotice`, `MemberSessionBindingChanged`
- To: `Stopped`

### `ObserveDestroyMemberRetirementArchivedRetiredRunning`
- From: `Running`
- On: `ObserveDestroyMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `session_ingress_detach_closed`
  - `destroy_admitted`
  - `identity_binding_matches`
  - `generation_matches`
  - `session_binding_matches`
  - `session_present`
  - `runtime_not_live`
  - `member_retiring`
  - `retirement_session_matches`
  - `kickoff_quiesced`
- Emits: `AppendLifecycleJournal`, `MemberSessionBindingChanged`
- To: `Running`

### `ObserveDestroyMemberRetirementArchivedRetiredStopped`
- From: `Stopped`
- On: `ObserveDestroyMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `session_ingress_detach_closed`
  - `destroy_admitted`
  - `identity_binding_matches`
  - `generation_matches`
  - `session_binding_matches`
  - `session_present`
  - `runtime_not_live`
  - `member_retiring`
  - `retirement_session_matches`
  - `kickoff_quiesced`
- Emits: `AppendLifecycleJournal`, `MemberSessionBindingChanged`
- To: `Stopped`

### `ObserveDestroyMemberRetirementArchivedAlreadyClearedRunning`
- From: `Running`
- On: `ObserveDestroyMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `destroy_admitted`
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `runtime_not_live`
  - `member_not_retiring`
  - `session_binding_cleared`
  - `retry_observes_cleared_session`
- Emits: `AppendLifecycleJournal`
- To: `Running`

### `ObserveDestroyMemberRetirementArchivedAlreadyClearedStopped`
- From: `Stopped`
- On: `ObserveDestroyMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `destroy_admitted`
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `runtime_not_live`
  - `member_not_retiring`
  - `session_binding_cleared`
  - `retry_observes_cleared_session`
- Emits: `AppendLifecycleJournal`
- To: `Stopped`

### `ResetMember`
- From: `Running`, `Stopped`
- On: `ResetMember`(agent_identity, agent_runtime_id, fence_token, generation, profile_name, runtime_mode, external_addressable, session_id)
- Guards:
  - `identity_binding_present`
  - `previous_runtime_session_ingress_detach_closed`
  - `previous_runtime_not_retiring`
  - `previous_runtime_retirement_closed`
  - `previous_runtime_retire_refusal_closed`
- Emits: `RequestRuntimeBinding`, `EmitMemberLifecycleNotice`
- To: `Running`

### `RespawnMember`
- From: `Running`
- On: `RespawnMember`(agent_identity, agent_runtime_id, fence_token, generation, profile_name, runtime_mode, external_addressable, session_id)
- Guards:
  - `identity_binding_present`
  - `previous_runtime_session_ingress_detach_closed`
  - `previous_runtime_not_retiring`
  - `previous_runtime_retirement_closed`
  - `previous_runtime_retire_refusal_closed`
- Emits: `RequestRuntimeBinding`, `EmitMemberLifecycleNotice`
- To: `Running`

### `ResolveRespawnTopologyRestoreCompleted`
- From: `Running`
- On: `ResolveRespawnTopologyRestore`(agent_identity, failed_peer_ids)
- Guards:
  - `identity_present`
  - `no_failed_peers`
- Emits: `RespawnTopologyRestoreResolved`
- To: `Running`

### `ResolveRespawnTopologyRestoreFailed`
- From: `Running`
- On: `ResolveRespawnTopologyRestore`(agent_identity, failed_peer_ids)
- Guards:
  - `identity_present`
  - `failed_peers_present`
- Emits: `RespawnTopologyRestoreResolved`
- To: `Running`

### `RecoverMemberRestoreFailureRunning`
- From: `Running`
- On: `RecoverMemberRestoreFailure`(agent_identity, reason)
- To: `Running`

### `ClassifyMemberLiveMaterializationRevivable`
- From: `Running`
- On: `ClassifyMemberLiveMaterialization`(agent_identity, observation, reason)
- Guards:
  - `identity_present`
  - `session_binding_present`
  - `not_broken`
  - `durable_snapshot_present`
- Emits: `MemberLiveMaterializationClassified`
- To: `Running`

### `ClassifyMemberLiveMaterializationTerminal`
- From: `Running`
- On: `ClassifyMemberLiveMaterialization`(agent_identity, observation, reason)
- Guards:
  - `identity_present`
  - `session_binding_present`
  - `not_broken`
  - `durable_snapshot_missing`
- Emits: `MemberLiveMaterializationClassified`
- To: `Running`

### `ResolveMemberRevivalSucceededRunning`
- From: `Running`
- On: `ResolveMemberRevivalSucceeded`(agent_identity)
- Guards:
  - `revival_pending`
- To: `Running`

### `ResolveMemberRevivalFailedRunning`
- From: `Running`
- On: `ResolveMemberRevivalFailed`(agent_identity, reason)
- Guards:
  - `revival_pending`
- To: `Running`

### `AdmitDestroyCleanup`
- From: `Running`, `Stopped`, `Completed`
- On: `AdmitDestroyCleanup`()
- Emits: `AppendLifecycleJournal`, `RequestPendingSpawnQuiesceForDestroy`
- To: `Running`

### `AdmitDestroyStorageFinalizing`
- From: `Destroyed`
- On: `AdmitDestroyStorageFinalizing`()
- Guards:
  - `destroy_admitted`
- Emits: `AppendLifecycleJournal`
- To: `Destroyed`

### `MarkCompleted`
- From: `Running`, `Stopped`
- On: `MarkCompleted`()
- Guards:
  - `no_active_runs`
- Emits: `EmitMemberLifecycleNotice`
- To: `Completed`

### `DestroyMob`
- From: `Running`, `Stopped`, `Completed`
- On: `DestroyMob`(session_id)
- Guards:
  - `session_ingress_detaches_closed`
  - `kickoff_waiters_quiesced`
  - `pending_spawns_drained`
- Emits: `RequestRuntimeDestroy`
- To: `Destroyed`

### `ObserveRuntimeDestroyed`
- From: `Running`, `Stopped`, `Completed`, `Destroyed`
- On: `ObserveRuntimeDestroyed`(agent_runtime_id, fence_token)
- Guards:
  - `session_ingress_detaches_closed`
  - `current_binding_matches`
  - `fence_token_matches`
- Emits: `EmitMemberLifecycleNotice`
- To: `Destroyed`

### `RecordOperatorActionProvenanceRunning`
- From: `Running`
- On: `RecordOperatorActionProvenance`(tool_name, principal_token, caller_provenance, audit_invocation_id)
- Emits: `AppendOperatorActionProvenance`
- To: `Running`

### `RecordOperatorActionProvenanceStopped`
- From: `Stopped`
- On: `RecordOperatorActionProvenance`(tool_name, principal_token, caller_provenance, audit_invocation_id)
- Emits: `AppendOperatorActionProvenance`
- To: `Stopped`

### `RecordOperatorActionProvenanceCompleted`
- From: `Completed`
- On: `RecordOperatorActionProvenance`(tool_name, principal_token, caller_provenance, audit_invocation_id)
- Emits: `AppendOperatorActionProvenance`
- To: `Completed`

### `RecordOperatorActionProvenanceDestroyed`
- From: `Destroyed`
- On: `RecordOperatorActionProvenance`(tool_name, principal_token, caller_provenance, audit_invocation_id)
- Emits: `AppendOperatorActionProvenance`
- To: `Destroyed`

### `SetSpawnPolicyRunning`
- From: `Running`
- On: `SetSpawnPolicy`(enabled)
- To: `Running`

### `SetSpawnPolicyStopped`
- From: `Stopped`
- On: `SetSpawnPolicy`(enabled)
- To: `Stopped`

### `SetSpawnPolicyCompleted`
- From: `Completed`
- On: `SetSpawnPolicy`(enabled)
- To: `Completed`

### `SetSpawnPolicyDestroyed`
- From: `Destroyed`
- On: `SetSpawnPolicy`(enabled)
- To: `Destroyed`

### `ResolveSpawnPolicyAdmitted`
- From: `Running`
- On: `ResolveSpawnPolicy`(agent_identity, revision, profile_name, runtime_mode)
- Guards:
  - `policy_enabled`
  - `revision_matches`
  - `identity_absent`
  - `profile_present`
- Emits: `SpawnPolicyResolutionRecorded`
- To: `Running`

### `ResolveSpawnPolicyNoMatch`
- From: `Running`
- On: `ResolveSpawnPolicy`(agent_identity, revision, profile_name, runtime_mode)
- Guards:
  - `policy_enabled`
  - `revision_matches`
  - `identity_absent`
  - `profile_absent`
  - `runtime_mode_absent`
- Emits: `SpawnPolicyResolutionRecorded`
- To: `Running`

### `BindOwnerBridgeSessionRunning`
- From: `Running`
- On: `BindOwnerBridgeSession`(bridge_session_id, destroy_on_owner_archive, implicit_delegation_mob)
- Guards:
  - `owner_bridge_session_absent`
  - `implicit_delegation_requires_cleanup`
- Emits: `OwnerBridgeSessionBound`
- To: `Running`

### `RecoverOwnerBridgeSessionRunning`
- From: `Running`
- On: `RecoverOwnerBridgeSession`(bridge_session_id, destroy_on_owner_archive, implicit_delegation_mob)
- Guards:
  - `owner_bridge_session_absent`
  - `implicit_delegation_requires_cleanup`
- To: `Running`

### `RecoverOwnerBridgeSessionAlreadyCurrent`
- From: `Running`
- On: `RecoverOwnerBridgeSession`(bridge_session_id, destroy_on_owner_archive, implicit_delegation_mob)
- Guards:
  - `owner_bridge_session_current`
  - `cleanup_policy_current`
  - `implicit_classification_current`
  - `implicit_delegation_requires_cleanup`
- To: `Running`

### `StopRunning`
- From: `Running`
- On: `Stop`()
- Guards:
  - `no_active_runs`
- Emits: `EmitRunLifecycleNotice`
- To: `Stopped`

### `ResumeStopped`
- From: `Stopped`
- On: `Resume`()
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `CompleteRunning`
- From: `Running`
- On: `Complete`()
- Emits: `AppendLifecycleJournal`, `EmitRunLifecycleNotice`
- To: `Completed`

### `ResetToRunning`
- From: `Running`, `Stopped`, `Completed`
- On: `Reset`()
- Emits: `AppendLifecycleJournal`, `EmitRunLifecycleNotice`
- To: `Running`

### `WireMembersRunning`
- From: `Running`
- On: `WireMembers`(edge)
- Guards:
  - `edge_not_already_wired`
- Emits: `WiringGraphChanged`, `EmitWiringLifecycleNotice`
- To: `Running`

### `WireMembersWithTrustRunning`
- From: `Running`
- On: `WireMembersWithTrust`(edge, a_identity, b_identity)
- Guards:
  - `edge_not_already_wired`
  - `edge_matches_members`
  - `a_member_peer_registered`
  - `b_member_peer_registered`
  - `a_member_endpoint_registered`
  - `b_member_endpoint_registered`
- Emits: `WiringGraphChanged`, `MemberTrustWiringRequested`, `EmitWiringLifecycleNotice`
- To: `Running`

### `WireMembersWithTrustAlreadyWired`
- From: `Running`
- On: `WireMembersWithTrust`(edge, a_identity, b_identity)
- Guards:
  - `edge_already_wired`
  - `edge_matches_members`
- Emits: `WiringTrustRepairRequested`
- To: `Running`

### `WireMembersAlreadyWired`
- From: `Running`
- On: `WireMembers`(edge)
- Guards:
  - `edge_already_wired`
- Emits: `WiringTrustRepairRequested`
- To: `Running`

### `RecoverRosterWiringRunning`
- From: `Running`
- On: `RecoverRosterWiring`(edge)
- Guards:
  - `edge_not_already_recovered`
- To: `Running`

### `RecoverRosterWiringAlreadyRecovered`
- From: `Running`
- On: `RecoverRosterWiring`(edge)
- Guards:
  - `edge_already_recovered`
- To: `Running`

### `RecoverRosterUnwireRunning`
- From: `Running`
- On: `RecoverRosterUnwire`(edge)
- Guards:
  - `edge_recovered`
- To: `Running`

### `RecoverRosterUnwireAlreadyAbsent`
- From: `Running`
- On: `RecoverRosterUnwire`(edge)
- Guards:
  - `edge_not_recovered`
- To: `Running`

### `ConvergeRecoveredRosterTopologyPruneRunning`
- From: `Running`
- On: `ConvergeRecoveredRosterTopology`(edge, a_identity, b_identity)
- Guards:
  - `edge_recovered`
  - `edge_matches_members`
  - `edge_has_absent_endpoint`
- To: `Running`

### `ConvergeRecoveredRosterTopologyPruneStopped`
- From: `Stopped`
- On: `ConvergeRecoveredRosterTopology`(edge, a_identity, b_identity)
- Guards:
  - `edge_recovered`
  - `edge_matches_members`
  - `edge_has_absent_endpoint`
- To: `Stopped`

### `ConvergeRecoveredRosterTopologyPruneCompleted`
- From: `Completed`
- On: `ConvergeRecoveredRosterTopology`(edge, a_identity, b_identity)
- Guards:
  - `edge_recovered`
  - `edge_matches_members`
  - `edge_has_absent_endpoint`
- To: `Completed`

### `ConvergeRecoveredRosterTopologyRetainRunning`
- From: `Running`
- On: `ConvergeRecoveredRosterTopology`(edge, a_identity, b_identity)
- Guards:
  - `edge_recovered`
  - `edge_matches_members`
  - `edge_endpoints_present`
- To: `Running`

### `ConvergeRecoveredRosterTopologyRetainStopped`
- From: `Stopped`
- On: `ConvergeRecoveredRosterTopology`(edge, a_identity, b_identity)
- Guards:
  - `edge_recovered`
  - `edge_matches_members`
  - `edge_endpoints_present`
- To: `Stopped`

### `ConvergeRecoveredRosterTopologyRetainCompleted`
- From: `Completed`
- On: `ConvergeRecoveredRosterTopology`(edge, a_identity, b_identity)
- Guards:
  - `edge_recovered`
  - `edge_matches_members`
  - `edge_endpoints_present`
- To: `Completed`

### `ConvergeRecoveredRosterTopologyDestroyed`
- From: `Destroyed`
- On: `ConvergeRecoveredRosterTopology`(edge, a_identity, b_identity)
- To: `Destroyed`

### `UnwireMembersRunning`
- From: `Running`
- On: `UnwireMembers`(edge)
- Guards:
  - `edge_currently_wired`
- Emits: `WiringGraphChanged`, `EmitWiringLifecycleNotice`
- To: `Running`

### `UnwireMembersAlreadyAbsent`
- From: `Running`
- On: `UnwireMembers`(edge)
- Guards:
  - `edge_already_absent`
- To: `Running`

### `CleanupRetiringMemberWiringRunning`
- From: `Running`
- On: `CleanupRetiringMemberWiring`(edge, a_identity, b_identity, agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `edge_currently_wired`
  - `edge_matches_members`
  - `retiring_identity_is_endpoint`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
- Emits: `WiringGraphChanged`
- To: `Running`

### `CleanupRetiringMemberWiringStopped`
- From: `Stopped`
- On: `CleanupRetiringMemberWiring`(edge, a_identity, b_identity, agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `edge_currently_wired`
  - `edge_matches_members`
  - `retiring_identity_is_endpoint`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
- Emits: `WiringGraphChanged`
- To: `Stopped`

### `CleanupRetiringMemberWiringAlreadyAbsentRunning`
- From: `Running`
- On: `CleanupRetiringMemberWiring`(edge, a_identity, b_identity, agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `edge_absent`
  - `edge_matches_members`
  - `retiring_identity_is_endpoint`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
- To: `Running`

### `CleanupRetiringMemberWiringAlreadyAbsentStopped`
- From: `Stopped`
- On: `CleanupRetiringMemberWiring`(edge, a_identity, b_identity, agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `edge_absent`
  - `edge_matches_members`
  - `retiring_identity_is_endpoint`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
- To: `Stopped`

### `RestoreRetiringMemberWiringRunning`
- From: `Running`
- On: `RestoreRetiringMemberWiring`(edge, a_identity, b_identity, agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `edge_absent`
  - `edge_matches_members`
  - `retiring_identity_is_endpoint`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
- Emits: `WiringGraphChanged`
- To: `Running`

### `RestoreRetiringMemberWiringStopped`
- From: `Stopped`
- On: `RestoreRetiringMemberWiring`(edge, a_identity, b_identity, agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `edge_absent`
  - `edge_matches_members`
  - `retiring_identity_is_endpoint`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
- Emits: `WiringGraphChanged`
- To: `Stopped`

### `WireExternalPeerRunning`
- From: `Running`
- On: `WireExternalPeer`(key, edge)
- Guards:
  - `external_peer_key_matches_edge`
  - `external_peer_key_not_already_wired`
  - `external_peer_edge_not_already_wired`
  - `local_member_peer_registered`
- Emits: `WiringGraphChanged`, `ExternalPeerTrustWiringRequested`, `EmitExternalPeerWiringLifecycleNotice`
- To: `Running`

### `WireExternalPeerAlreadyWired`
- From: `Running`
- On: `WireExternalPeer`(key, edge)
- Guards:
  - `external_peer_key_matches_edge`
  - `external_peer_key_already_wired`
  - `external_peer_edge_already_wired`
  - `local_member_peer_registered`
- Emits: `ExternalPeerTrustRepairRequested`
- To: `Running`

### `RegisterMemberPeerRunning`
- From: `Running`
- On: `RegisterMemberPeer`(agent_identity, peer_endpoint)
- Guards:
  - `identity_present`
  - `member_peer_not_registered`
  - `member_endpoint_not_registered`
  - `fresh_spawn_or_no_retained_topology`
  - `peer_id_available_for_identity`
- Emits: `MemberPeerRegistered`
- To: `Running`

### `RegisterMemberPeerAlreadyCurrentRunning`
- From: `Running`
- On: `RegisterMemberPeer`(agent_identity, peer_endpoint)
- Guards:
  - `identity_present`
  - `member_peer_registered`
  - `member_endpoint_registered`
  - `current_endpoint_matches`
  - `current_peer_id_matches`
- Emits: `MemberPeerRegistered`
- To: `Running`

### `AuthorizeMemberEndpointMigrationTrustCleanupRunningRunning`
- From: `Running`
- On: `AuthorizeMemberEndpointMigrationTrustCleanup`(edge, agent_identity, agent_runtime_id, retained_peer_endpoint)
- Guards:
  - `identity_runtime_matches`
  - `runtime_live_or_retiring`
  - `edge_currently_wired`
  - `edge_contains_migrated_member`
  - `retained_endpoint_history_present`
  - `migrated_current_endpoint_registered`
  - `migrated_current_peer_registered`
  - `migrated_current_endpoint_changed`
  - `migrated_current_peer_id_changed`
  - `migrated_current_peer_id_matches_endpoint`
  - `edge_a_current_peer_registered`
  - `edge_b_current_peer_registered`
  - `edge_a_current_endpoint_registered`
  - `edge_b_current_endpoint_registered`
  - `edge_a_current_peer_id_matches_endpoint`
  - `edge_b_current_peer_id_matches_endpoint`
- Emits: `MemberTrustUnwiringRequested`
- To: `Running`

### `AuthorizeMemberEndpointMigrationTrustCleanupRunningStopped`
- From: `Stopped`
- On: `AuthorizeMemberEndpointMigrationTrustCleanup`(edge, agent_identity, agent_runtime_id, retained_peer_endpoint)
- Guards:
  - `identity_runtime_matches`
  - `runtime_live_or_retiring`
  - `edge_currently_wired`
  - `edge_contains_migrated_member`
  - `retained_endpoint_history_present`
  - `migrated_current_endpoint_registered`
  - `migrated_current_peer_registered`
  - `migrated_current_endpoint_changed`
  - `migrated_current_peer_id_changed`
  - `migrated_current_peer_id_matches_endpoint`
  - `edge_a_current_peer_registered`
  - `edge_b_current_peer_registered`
  - `edge_a_current_endpoint_registered`
  - `edge_b_current_endpoint_registered`
  - `edge_a_current_peer_id_matches_endpoint`
  - `edge_b_current_peer_id_matches_endpoint`
- Emits: `MemberTrustUnwiringRequested`
- To: `Stopped`

### `AuthorizeMemberPeerRebindRunning`
- From: `Running`
- On: `AuthorizeMemberPeerRebind`(agent_identity, expected_peer_endpoint)
- Guards:
  - `identity_present`
  - `member_peer_registered`
  - `member_endpoint_registered`
  - `current_endpoint_matches_expected`
  - `current_peer_id_matches_expected`
- Emits: `MemberPeerRebindAuthorized`
- To: `Running`

### `AuthorizeMemberPeerOverlayRunning`
- From: `Running`
- On: `AuthorizeMemberPeerOverlay`(agent_identity, expected_peer_endpoint)
- Guards:
  - `identity_present`
  - `member_peer_registered`
  - `member_endpoint_registered`
  - `current_endpoint_matches_expected`
  - `current_peer_id_matches_expected`
  - `overlay_member_endpoints_complete`
  - `overlay_peer_ids_unique`
- Emits: `MemberPeerOverlayAuthorized`
- To: `Running`

### `AuthorizeMemberTrustWiringRunning`
- From: `Running`
- On: `AuthorizeMemberTrustWiring`(edge, a_identity, b_identity)
- Guards:
  - `edge_currently_wired`
  - `edge_matches_members`
  - `a_member_peer_registered`
  - `b_member_peer_registered`
  - `a_member_endpoint_registered`
  - `b_member_endpoint_registered`
- Emits: `MemberTrustWiringRequested`
- To: `Running`

### `AuthorizeMemberTrustUnwiringRunning`
- From: `Running`
- On: `AuthorizeMemberTrustUnwiring`(edge, a_identity, b_identity)
- Guards:
  - `edge_currently_wired`
  - `edge_matches_members`
  - `a_member_peer_registered`
  - `b_member_peer_registered`
- Emits: `MemberTrustUnwiringRequested`
- To: `Running`

### `AuthorizeMemberTrustUnwiringStopped`
- From: `Stopped`
- On: `AuthorizeMemberTrustUnwiring`(edge, a_identity, b_identity)
- Guards:
  - `edge_currently_wired`
  - `edge_matches_members`
  - `a_member_peer_registered`
  - `b_member_peer_registered`
- Emits: `MemberTrustUnwiringRequested`
- To: `Stopped`

### `AuthorizeMemberTrustCleanupRunning`
- From: `Running`
- On: `AuthorizeMemberTrustCleanup`(edge, a_identity, b_identity)
- Guards:
  - `edge_matches_members`
  - `a_member_peer_registered`
  - `b_member_peer_registered`
- Emits: `MemberTrustUnwiringRequested`
- To: `Running`

### `AuthorizeMemberTrustCleanupStopped`
- From: `Stopped`
- On: `AuthorizeMemberTrustCleanup`(edge, a_identity, b_identity)
- Guards:
  - `edge_matches_members`
  - `a_member_peer_registered`
  - `b_member_peer_registered`
- Emits: `MemberTrustUnwiringRequested`
- To: `Stopped`

### `AuthorizeMemberTrustCleanupObservedRunning`
- From: `Running`
- On: `AuthorizeMemberTrustCleanupObserved`(edge, a_identity, a_peer_id, b_identity, b_peer_id)
- Guards:
  - `edge_matches_members`
  - `edge_currently_wired`
  - `cleanup_has_restore_failure`
- Emits: `MemberTrustUnwiringRequested`
- To: `Running`

### `AuthorizeMemberTrustCleanupObservedStopped`
- From: `Stopped`
- On: `AuthorizeMemberTrustCleanupObserved`(edge, a_identity, a_peer_id, b_identity, b_peer_id)
- Guards:
  - `edge_matches_members`
  - `edge_currently_wired`
  - `cleanup_has_restore_failure`
- Emits: `MemberTrustUnwiringRequested`
- To: `Stopped`

### `AuthorizeRetiringMemberTrustCleanupObservedRunning`
- From: `Running`
- On: `AuthorizeRetiringMemberTrustCleanupObserved`(edge, a_identity, a_peer_id, b_identity, b_peer_id, agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `edge_matches_members`
  - `edge_currently_wired`
  - `retiring_identity_is_endpoint`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
- Emits: `MemberTrustUnwiringRequested`
- To: `Running`

### `AuthorizeRetiringMemberTrustCleanupObservedStopped`
- From: `Stopped`
- On: `AuthorizeRetiringMemberTrustCleanupObserved`(edge, a_identity, a_peer_id, b_identity, b_peer_id, agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `edge_matches_members`
  - `edge_currently_wired`
  - `retiring_identity_is_endpoint`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
- Emits: `MemberTrustUnwiringRequested`
- To: `Stopped`

### `AuthorizeExternalPeerReciprocalTrustRunning`
- From: `Running`
- On: `AuthorizeExternalPeerReciprocalTrust`(key, agent_identity)
- Guards:
  - `external_peer_key_already_wired`
  - `external_peer_key_matches_member`
  - `member_peer_registered`
  - `member_endpoint_registered`
- Emits: `ExternalPeerReciprocalTrustRequested`
- To: `Running`

### `RecoverExternalPeerWiringRunning`
- From: `Running`
- On: `RecoverExternalPeerWiring`(key, edge)
- Guards:
  - `external_peer_key_matches_edge`
  - `external_peer_key_not_already_recovered`
  - `external_peer_edge_not_already_recovered`
- To: `Running`

### `RecoverExternalPeerWiringAlreadyRecovered`
- From: `Running`
- On: `RecoverExternalPeerWiring`(key, edge)
- Guards:
  - `external_peer_key_matches_edge`
  - `external_peer_key_already_recovered`
  - `external_peer_edge_already_recovered`
- To: `Running`

### `RecoverExternalPeerUnwireRunning`
- From: `Running`
- On: `RecoverExternalPeerUnwire`(key)
- Guards:
  - `external_peer_recovered`
- To: `Running`

### `RecoverExternalPeerUnwireAlreadyAbsent`
- From: `Running`
- On: `RecoverExternalPeerUnwire`(key)
- Guards:
  - `external_peer_not_recovered`
- To: `Running`

### `UnwireExternalPeerRunning`
- From: `Running`
- On: `UnwireExternalPeer`(key, edge)
- Guards:
  - `external_peer_key_matches_edge`
  - `external_peer_key_currently_wired`
  - `external_peer_edge_currently_wired`
  - `local_member_peer_registered`
- Emits: `WiringGraphChanged`, `ExternalPeerTrustUnwiringRequested`, `EmitExternalPeerWiringLifecycleNotice`
- To: `Running`

### `UnwireExternalPeerAlreadyAbsent`
- From: `Running`
- On: `UnwireExternalPeer`(key, edge)
- Guards:
  - `external_peer_key_matches_edge`
  - `external_peer_key_already_absent`
  - `external_peer_edge_already_absent`
- To: `Running`

### `CleanupRetiringExternalPeerRunning`
- From: `Running`
- On: `CleanupRetiringExternalPeer`(key, edge, agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `external_peer_key_matches_edge`
  - `external_peer_key_matches_member`
  - `external_peer_key_currently_wired`
  - `external_peer_edge_currently_wired`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `local_member_peer_registered`
- Emits: `WiringGraphChanged`, `ExternalPeerTrustUnwiringRequested`
- To: `Running`

### `CleanupRetiringExternalPeerStopped`
- From: `Stopped`
- On: `CleanupRetiringExternalPeer`(key, edge, agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `external_peer_key_matches_edge`
  - `external_peer_key_matches_member`
  - `external_peer_key_currently_wired`
  - `external_peer_edge_currently_wired`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `local_member_peer_registered`
- Emits: `WiringGraphChanged`, `ExternalPeerTrustUnwiringRequested`
- To: `Stopped`

### `RestoreRetiringExternalPeerRunning`
- From: `Running`
- On: `RestoreRetiringExternalPeer`(key, edge, agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `external_peer_key_matches_edge`
  - `external_peer_key_matches_member`
  - `external_peer_key_absent`
  - `external_peer_edge_absent`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `local_member_peer_registered`
- Emits: `WiringGraphChanged`, `ExternalPeerTrustWiringRequested`
- To: `Running`

### `RestoreRetiringExternalPeerStopped`
- From: `Stopped`
- On: `RestoreRetiringExternalPeer`(key, edge, agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `external_peer_key_matches_edge`
  - `external_peer_key_matches_member`
  - `external_peer_key_absent`
  - `external_peer_edge_absent`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `local_member_peer_registered`
- Emits: `WiringGraphChanged`, `ExternalPeerTrustWiringRequested`
- To: `Stopped`

### `CleanupRetiringExternalPeerObservedAbsentRunning`
- From: `Running`
- On: `CleanupRetiringExternalPeerObservedAbsent`(key, edge, agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `external_peer_key_matches_edge`
  - `external_peer_key_matches_member`
  - `external_peer_key_currently_wired`
  - `external_peer_edge_currently_wired`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `retiring_peer_endpoint_retained`
  - `retiring_peer_id_retained`
- Emits: `WiringGraphChanged`
- To: `Running`

### `CleanupRetiringExternalPeerObservedAbsentStopped`
- From: `Stopped`
- On: `CleanupRetiringExternalPeerObservedAbsent`(key, edge, agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `external_peer_key_matches_edge`
  - `external_peer_key_matches_member`
  - `external_peer_key_currently_wired`
  - `external_peer_edge_currently_wired`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `retiring_peer_endpoint_retained`
  - `retiring_peer_id_retained`
- Emits: `WiringGraphChanged`
- To: `Stopped`

### `RestoreRetiringExternalPeerObservedAbsentRunning`
- From: `Running`
- On: `RestoreRetiringExternalPeerObservedAbsent`(key, edge, agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `external_peer_key_matches_edge`
  - `external_peer_key_matches_member`
  - `external_peer_key_absent`
  - `external_peer_edge_absent`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `retiring_peer_endpoint_retained`
  - `retiring_peer_id_retained`
- Emits: `WiringGraphChanged`
- To: `Running`

### `RestoreRetiringExternalPeerObservedAbsentStopped`
- From: `Stopped`
- On: `RestoreRetiringExternalPeerObservedAbsent`(key, edge, agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `external_peer_key_matches_edge`
  - `external_peer_key_matches_member`
  - `external_peer_key_absent`
  - `external_peer_edge_absent`
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `retiring_peer_endpoint_retained`
  - `retiring_peer_id_retained`
- Emits: `WiringGraphChanged`
- To: `Stopped`

### `AdmitSupervisorRotationRunning`
- From: `Running`
- On: `AdmitSupervisorRotation`()
- Guards:
  - `destroy_not_admitted`
  - `no_member_retiring`
- To: `Running`

### `AdmitSupervisorRotationStopped`
- From: `Stopped`
- On: `AdmitSupervisorRotation`()
- Guards:
  - `destroy_not_admitted`
  - `no_member_retiring`
- To: `Stopped`

### `ProvisionSupervisorAuthorityRunning`
- From: `Running`
- On: `ProvisionSupervisorAuthority`(peer_id, signing_key, epoch, protocol_version)
- Guards:
  - `supervisor_authority_absent`
  - `initial_epoch`
- Emits: `PersistSupervisorAuthority`
- To: `Running`

### `ProvisionSupervisorAuthorityStopped`
- From: `Stopped`
- On: `ProvisionSupervisorAuthority`(peer_id, signing_key, epoch, protocol_version)
- Guards:
  - `supervisor_authority_absent`
  - `initial_epoch`
- Emits: `PersistSupervisorAuthority`
- To: `Stopped`

### `ProvisionSupervisorAuthorityCompleted`
- From: `Completed`
- On: `ProvisionSupervisorAuthority`(peer_id, signing_key, epoch, protocol_version)
- Guards:
  - `supervisor_authority_absent`
  - `initial_epoch`
- Emits: `PersistSupervisorAuthority`
- To: `Completed`

### `ProvisionSupervisorAuthorityDestroyed`
- From: `Destroyed`
- On: `ProvisionSupervisorAuthority`(peer_id, signing_key, epoch, protocol_version)
- Guards:
  - `supervisor_authority_absent`
  - `initial_epoch`
- Emits: `PersistSupervisorAuthority`
- To: `Destroyed`

### `RecoverSupervisorAuthorityRunning`
- From: `Running`
- On: `RecoverSupervisorAuthority`(peer_id, signing_key, epoch, protocol_version, pending_operation_id, pending_peer_id, pending_signing_key, pending_epoch, pending_protocol_version, pending_accepted_peer_ids, pending_member_target_names, pending_member_target_addresses)
- Guards:
  - `supervisor_authority_absent`
  - `pending_shape_consistent`
  - `pending_member_targets_aligned`
- To: `Running`

### `RecoverSupervisorAuthorityStopped`
- From: `Stopped`
- On: `RecoverSupervisorAuthority`(peer_id, signing_key, epoch, protocol_version, pending_operation_id, pending_peer_id, pending_signing_key, pending_epoch, pending_protocol_version, pending_accepted_peer_ids, pending_member_target_names, pending_member_target_addresses)
- Guards:
  - `supervisor_authority_absent`
  - `pending_shape_consistent`
  - `pending_member_targets_aligned`
- To: `Stopped`

### `RecoverSupervisorAuthorityCompleted`
- From: `Completed`
- On: `RecoverSupervisorAuthority`(peer_id, signing_key, epoch, protocol_version, pending_operation_id, pending_peer_id, pending_signing_key, pending_epoch, pending_protocol_version, pending_accepted_peer_ids, pending_member_target_names, pending_member_target_addresses)
- Guards:
  - `supervisor_authority_absent`
  - `pending_shape_consistent`
  - `pending_member_targets_aligned`
- To: `Completed`

### `RecoverSupervisorAuthorityDestroyed`
- From: `Destroyed`
- On: `RecoverSupervisorAuthority`(peer_id, signing_key, epoch, protocol_version, pending_operation_id, pending_peer_id, pending_signing_key, pending_epoch, pending_protocol_version, pending_accepted_peer_ids, pending_member_target_names, pending_member_target_addresses)
- Guards:
  - `supervisor_authority_absent`
  - `pending_shape_consistent`
  - `pending_member_targets_aligned`
- To: `Destroyed`

### `RecordSupervisorPendingRotationRunning`
- From: `Running`
- On: `RecordSupervisorPendingRotation`(current_peer_id, current_epoch, current_protocol_version, operation_id, pending_peer_id, pending_signing_key, pending_epoch, pending_protocol_version, accepted_peer_ids, active_peer_ids, member_target_names, member_target_addresses)
- Guards:
  - `current_supervisor_matches`
  - `pending_authority_changes_peer`
  - `pending_epoch_is_successor`
  - `operation_id_not_empty`
  - `member_targets_aligned`
  - `accepted_peers_have_targets`
  - `pending_slot_empty_legacy_reconciled_or_exact_retry`
- Emits: `PersistSupervisorAuthority`
- To: `Running`

### `RecordSupervisorPendingRotationStopped`
- From: `Stopped`
- On: `RecordSupervisorPendingRotation`(current_peer_id, current_epoch, current_protocol_version, operation_id, pending_peer_id, pending_signing_key, pending_epoch, pending_protocol_version, accepted_peer_ids, active_peer_ids, member_target_names, member_target_addresses)
- Guards:
  - `current_supervisor_matches`
  - `pending_authority_changes_peer`
  - `pending_epoch_is_successor`
  - `operation_id_not_empty`
  - `member_targets_aligned`
  - `accepted_peers_have_targets`
  - `pending_slot_empty_legacy_reconciled_or_exact_retry`
- Emits: `PersistSupervisorAuthority`
- To: `Stopped`

### `RecordSupervisorPendingRotationCompleted`
- From: `Completed`
- On: `RecordSupervisorPendingRotation`(current_peer_id, current_epoch, current_protocol_version, operation_id, pending_peer_id, pending_signing_key, pending_epoch, pending_protocol_version, accepted_peer_ids, active_peer_ids, member_target_names, member_target_addresses)
- Guards:
  - `current_supervisor_matches`
  - `pending_authority_changes_peer`
  - `pending_epoch_is_successor`
  - `operation_id_not_empty`
  - `member_targets_aligned`
  - `accepted_peers_have_targets`
  - `pending_slot_empty_legacy_reconciled_or_exact_retry`
- Emits: `PersistSupervisorAuthority`
- To: `Completed`

### `RecordPendingRecipientTrustRunning`
- From: `Running`
- On: `RecordPendingRecipientTrust`(peer_id)
- To: `Running`

### `RecordPendingRecipientTrustStopped`
- From: `Stopped`
- On: `RecordPendingRecipientTrust`(peer_id)
- To: `Stopped`

### `RecordPendingRecipientTrustCompleted`
- From: `Completed`
- On: `RecordPendingRecipientTrust`(peer_id)
- To: `Completed`

### `RecordPendingRecipientTrustDestroyed`
- From: `Destroyed`
- On: `RecordPendingRecipientTrust`(peer_id)
- To: `Destroyed`

### `ResolvePendingRecipientTrustRunning`
- From: `Running`
- On: `ResolvePendingRecipientTrust`(peer_id)
- To: `Running`

### `ResolvePendingRecipientTrustStopped`
- From: `Stopped`
- On: `ResolvePendingRecipientTrust`(peer_id)
- To: `Stopped`

### `ResolvePendingRecipientTrustCompleted`
- From: `Completed`
- On: `ResolvePendingRecipientTrust`(peer_id)
- To: `Completed`

### `ResolvePendingRecipientTrustDestroyed`
- From: `Destroyed`
- On: `ResolvePendingRecipientTrust`(peer_id)
- To: `Destroyed`

### `RollbackPendingRecipientTrustRunning`
- From: `Running`
- On: `RollbackPendingRecipientTrust`(peer_id)
- To: `Running`

### `RollbackPendingRecipientTrustStopped`
- From: `Stopped`
- On: `RollbackPendingRecipientTrust`(peer_id)
- To: `Stopped`

### `RollbackPendingRecipientTrustCompleted`
- From: `Completed`
- On: `RollbackPendingRecipientTrust`(peer_id)
- To: `Completed`

### `RollbackPendingRecipientTrustDestroyed`
- From: `Destroyed`
- On: `RollbackPendingRecipientTrust`(peer_id)
- To: `Destroyed`

### `CommitSupervisorRotationRunning`
- From: `Running`
- On: `CommitSupervisorRotation`(current_peer_id, current_epoch, current_protocol_version, operation_id, next_peer_id, next_signing_key, next_epoch, next_protocol_version)
- Guards:
  - `current_supervisor_matches`
  - `next_epoch_is_successor`
  - `next_authority_changes_peer`
  - `pending_operation_matches`
  - `pending_authority_matches_next`
- Emits: `PersistSupervisorAuthority`
- To: `Running`

### `CommitSupervisorRotationStopped`
- From: `Stopped`
- On: `CommitSupervisorRotation`(current_peer_id, current_epoch, current_protocol_version, operation_id, next_peer_id, next_signing_key, next_epoch, next_protocol_version)
- Guards:
  - `current_supervisor_matches`
  - `next_epoch_is_successor`
  - `next_authority_changes_peer`
  - `pending_operation_matches`
  - `pending_authority_matches_next`
- Emits: `PersistSupervisorAuthority`
- To: `Stopped`

### `CommitSupervisorRotationCompleted`
- From: `Completed`
- On: `CommitSupervisorRotation`(current_peer_id, current_epoch, current_protocol_version, operation_id, next_peer_id, next_signing_key, next_epoch, next_protocol_version)
- Guards:
  - `current_supervisor_matches`
  - `next_epoch_is_successor`
  - `next_authority_changes_peer`
  - `pending_operation_matches`
  - `pending_authority_matches_next`
- Emits: `PersistSupervisorAuthority`
- To: `Completed`

### `ClearSupervisorAuthorityForDestroy`
- From: `Destroyed`
- On: `ClearSupervisorAuthorityForDestroy`(current_peer_id, current_signing_key, current_epoch, protocol_version)
- Guards:
  - `current_supervisor_matches`
- Emits: `DeleteSupervisorAuthority`
- To: `Destroyed`

### `RestoreSupervisorAuthorityAfterDestroyRollback`
- From: `Destroyed`
- On: `RestoreSupervisorAuthorityAfterDestroyRollback`(peer_id, signing_key, epoch, protocol_version, pending_operation_id, pending_peer_id, pending_signing_key, pending_epoch, pending_protocol_version, pending_accepted_peer_ids, pending_member_target_names, pending_member_target_addresses)
- Guards:
  - `supervisor_authority_absent`
  - `pending_shape_consistent`
  - `pending_member_targets_aligned`
- Emits: `PersistSupervisorAuthority`
- To: `Destroyed`

### `ForceCancelRunning`
- From: `Running`
- On: `ForceCancel`(agent_identity)
- Guards:
  - `identity_known`
  - `runtime_live`
  - `member_not_retiring`
- Emits: `FlowTerminalized`
- To: `Running`

### `SubscribeAgentEventsRunning`
- From: `Running`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_live`
  - `session_bound`
- Emits: `AuthorizeAgentEventSubscription`
- To: `Running`

### `SubscribeAgentEventsStopped`
- From: `Stopped`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_live`
  - `session_bound`
- Emits: `AuthorizeAgentEventSubscription`
- To: `Stopped`

### `SubscribeAgentEventsCompleted`
- From: `Completed`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_live`
  - `session_bound`
- Emits: `AuthorizeAgentEventSubscription`
- To: `Completed`

### `SubscribeAgentEventsDestroyed`
- From: `Destroyed`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_live`
  - `session_bound`
- Emits: `AuthorizeAgentEventSubscription`
- To: `Destroyed`

### `SubscribeAgentEventsMissingMemberRunning`
- From: `Running`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_absent`
- Emits: `RejectAgentEventSubscription`
- To: `Running`

### `SubscribeAgentEventsMissingMemberStopped`
- From: `Stopped`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_absent`
- Emits: `RejectAgentEventSubscription`
- To: `Stopped`

### `SubscribeAgentEventsMissingMemberCompleted`
- From: `Completed`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_absent`
- Emits: `RejectAgentEventSubscription`
- To: `Completed`

### `SubscribeAgentEventsMissingMemberDestroyed`
- From: `Destroyed`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_absent`
- Emits: `RejectAgentEventSubscription`
- To: `Destroyed`

### `SubscribeAgentEventsMissingSessionRunning`
- From: `Running`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_live`
  - `session_unbound`
- Emits: `RejectAgentEventSubscription`
- To: `Running`

### `SubscribeAgentEventsMissingSessionStopped`
- From: `Stopped`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_live`
  - `session_unbound`
- Emits: `RejectAgentEventSubscription`
- To: `Stopped`

### `SubscribeAgentEventsMissingSessionCompleted`
- From: `Completed`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_live`
  - `session_unbound`
- Emits: `RejectAgentEventSubscription`
- To: `Completed`

### `SubscribeAgentEventsMissingSessionDestroyed`
- From: `Destroyed`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_live`
  - `session_unbound`
- Emits: `RejectAgentEventSubscription`
- To: `Destroyed`

### `SubscribeAgentEventsRuntimeNotLiveRunning`
- From: `Running`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_not_live`
- Emits: `RejectAgentEventSubscription`
- To: `Running`

### `SubscribeAgentEventsRuntimeNotLiveStopped`
- From: `Stopped`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_not_live`
- Emits: `RejectAgentEventSubscription`
- To: `Stopped`

### `SubscribeAgentEventsRuntimeNotLiveCompleted`
- From: `Completed`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_not_live`
- Emits: `RejectAgentEventSubscription`
- To: `Completed`

### `SubscribeAgentEventsRuntimeNotLiveDestroyed`
- From: `Destroyed`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_not_live`
- Emits: `RejectAgentEventSubscription`
- To: `Destroyed`

### `SubscribeAllAgentEventsRunning`
- From: `Running`
- On: `SubscribeAllAgentEvents`(session_bound_runtimes)
- Guards:
  - `session_bound_runtimes_match`
  - `session_bound_or_no_live_members`
- Emits: `AuthorizeAllAgentEventSubscription`
- To: `Running`

### `SubscribeAllAgentEventsStopped`
- From: `Stopped`
- On: `SubscribeAllAgentEvents`(session_bound_runtimes)
- Guards:
  - `session_bound_runtimes_match`
  - `session_bound_or_no_live_members`
- Emits: `AuthorizeAllAgentEventSubscription`
- To: `Stopped`

### `SubscribeAllAgentEventsCompleted`
- From: `Completed`
- On: `SubscribeAllAgentEvents`(session_bound_runtimes)
- Guards:
  - `session_bound_runtimes_match`
  - `session_bound_or_no_live_members`
- Emits: `AuthorizeAllAgentEventSubscription`
- To: `Completed`

### `SubscribeAllAgentEventsDestroyed`
- From: `Destroyed`
- On: `SubscribeAllAgentEvents`(session_bound_runtimes)
- Guards:
  - `session_bound_runtimes_match`
  - `session_bound_or_no_live_members`
- Emits: `AuthorizeAllAgentEventSubscription`
- To: `Destroyed`

### `SubscribeAllAgentEventsNoSessionBindingsRunning`
- From: `Running`
- On: `SubscribeAllAgentEvents`(session_bound_runtimes)
- Guards:
  - `session_bound_runtimes_match`
  - `no_session_bound_runtime`
  - `live_members_present`
- Emits: `RejectAllAgentEventSubscription`
- To: `Running`

### `SubscribeAllAgentEventsNoSessionBindingsStopped`
- From: `Stopped`
- On: `SubscribeAllAgentEvents`(session_bound_runtimes)
- Guards:
  - `session_bound_runtimes_match`
  - `no_session_bound_runtime`
  - `live_members_present`
- Emits: `RejectAllAgentEventSubscription`
- To: `Stopped`

### `SubscribeAllAgentEventsNoSessionBindingsCompleted`
- From: `Completed`
- On: `SubscribeAllAgentEvents`(session_bound_runtimes)
- Guards:
  - `session_bound_runtimes_match`
  - `no_session_bound_runtime`
  - `live_members_present`
- Emits: `RejectAllAgentEventSubscription`
- To: `Completed`

### `SubscribeAllAgentEventsNoSessionBindingsDestroyed`
- From: `Destroyed`
- On: `SubscribeAllAgentEvents`(session_bound_runtimes)
- Guards:
  - `session_bound_runtimes_match`
  - `no_session_bound_runtime`
  - `live_members_present`
- Emits: `RejectAllAgentEventSubscription`
- To: `Destroyed`

### `SubscribeMobEventsRunning`
- From: `Running`
- On: `SubscribeMobEvents`(initial_cursor, channel_capacity, poll_interval_ms, session_bound_runtimes)
- Guards:
  - `channel_capacity_positive`
  - `poll_interval_positive`
  - `session_bound_runtimes_match`
- Emits: `AuthorizeMobEventRouter`
- To: `Running`

### `SubscribeMobEventsStopped`
- From: `Stopped`
- On: `SubscribeMobEvents`(initial_cursor, channel_capacity, poll_interval_ms, session_bound_runtimes)
- Guards:
  - `channel_capacity_positive`
  - `poll_interval_positive`
  - `session_bound_runtimes_match`
- Emits: `AuthorizeMobEventRouter`
- To: `Stopped`

### `SubscribeMobEventsCompleted`
- From: `Completed`
- On: `SubscribeMobEvents`(initial_cursor, channel_capacity, poll_interval_ms, session_bound_runtimes)
- Guards:
  - `channel_capacity_positive`
  - `poll_interval_positive`
  - `session_bound_runtimes_match`
- Emits: `AuthorizeMobEventRouter`
- To: `Completed`

### `SubscribeMobEventsDestroyed`
- From: `Destroyed`
- On: `SubscribeMobEvents`(initial_cursor, channel_capacity, poll_interval_ms, session_bound_runtimes)
- Guards:
  - `channel_capacity_positive`
  - `poll_interval_positive`
  - `session_bound_runtimes_match`
- Emits: `AuthorizeMobEventRouter`
- To: `Destroyed`

### `SubscribeStructuralEventsRunning`
- From: `Running`
- On: `SubscribeStructuralEvents`(after_cursor, latest_cursor, explicit_after_cursor, batch_limit, channel_capacity)
- Guards:
  - `cursor_not_stale`
  - `batch_limit_positive`
  - `channel_capacity_positive`
- Emits: `AuthorizeStructuralEventSubscription`
- To: `Running`

### `SubscribeStructuralEventsStopped`
- From: `Stopped`
- On: `SubscribeStructuralEvents`(after_cursor, latest_cursor, explicit_after_cursor, batch_limit, channel_capacity)
- Guards:
  - `cursor_not_stale`
  - `batch_limit_positive`
  - `channel_capacity_positive`
- Emits: `AuthorizeStructuralEventSubscription`
- To: `Stopped`

### `SubscribeStructuralEventsCompleted`
- From: `Completed`
- On: `SubscribeStructuralEvents`(after_cursor, latest_cursor, explicit_after_cursor, batch_limit, channel_capacity)
- Guards:
  - `cursor_not_stale`
  - `batch_limit_positive`
  - `channel_capacity_positive`
- Emits: `AuthorizeStructuralEventSubscription`
- To: `Completed`

### `SubscribeStructuralEventsDestroyed`
- From: `Destroyed`
- On: `SubscribeStructuralEvents`(after_cursor, latest_cursor, explicit_after_cursor, batch_limit, channel_capacity)
- Guards:
  - `cursor_not_stale`
  - `batch_limit_positive`
  - `channel_capacity_positive`
- Emits: `AuthorizeStructuralEventSubscription`
- To: `Destroyed`

### `SubscribeStructuralEventsStaleRunning`
- From: `Running`
- On: `SubscribeStructuralEvents`(after_cursor, latest_cursor, explicit_after_cursor, batch_limit, channel_capacity)
- Guards:
  - `cursor_stale`
- Emits: `RejectStructuralEventSubscription`
- To: `Running`

### `SubscribeStructuralEventsStaleStopped`
- From: `Stopped`
- On: `SubscribeStructuralEvents`(after_cursor, latest_cursor, explicit_after_cursor, batch_limit, channel_capacity)
- Guards:
  - `cursor_stale`
- Emits: `RejectStructuralEventSubscription`
- To: `Stopped`

### `SubscribeStructuralEventsStaleCompleted`
- From: `Completed`
- On: `SubscribeStructuralEvents`(after_cursor, latest_cursor, explicit_after_cursor, batch_limit, channel_capacity)
- Guards:
  - `cursor_stale`
- Emits: `RejectStructuralEventSubscription`
- To: `Completed`

### `SubscribeStructuralEventsStaleDestroyed`
- From: `Destroyed`
- On: `SubscribeStructuralEvents`(after_cursor, latest_cursor, explicit_after_cursor, batch_limit, channel_capacity)
- Guards:
  - `cursor_stale`
- Emits: `RejectStructuralEventSubscription`
- To: `Destroyed`

### `PollEventsStrictRunning`
- From: `Running`
- On: `PollEventsStrict`(after_cursor, latest_cursor, limit)
- Guards:
  - `cursor_not_stale`
- Emits: `AuthorizeStrictEventPoll`
- To: `Running`

### `PollEventsStrictStopped`
- From: `Stopped`
- On: `PollEventsStrict`(after_cursor, latest_cursor, limit)
- Guards:
  - `cursor_not_stale`
- Emits: `AuthorizeStrictEventPoll`
- To: `Stopped`

### `PollEventsStrictCompleted`
- From: `Completed`
- On: `PollEventsStrict`(after_cursor, latest_cursor, limit)
- Guards:
  - `cursor_not_stale`
- Emits: `AuthorizeStrictEventPoll`
- To: `Completed`

### `PollEventsStrictDestroyed`
- From: `Destroyed`
- On: `PollEventsStrict`(after_cursor, latest_cursor, limit)
- Guards:
  - `cursor_not_stale`
- Emits: `AuthorizeStrictEventPoll`
- To: `Destroyed`

### `PollEventsStrictStaleRunning`
- From: `Running`
- On: `PollEventsStrict`(after_cursor, latest_cursor, limit)
- Guards:
  - `cursor_stale`
- Emits: `RejectStrictEventPoll`
- To: `Running`

### `PollEventsStrictStaleStopped`
- From: `Stopped`
- On: `PollEventsStrict`(after_cursor, latest_cursor, limit)
- Guards:
  - `cursor_stale`
- Emits: `RejectStrictEventPoll`
- To: `Stopped`

### `PollEventsStrictStaleCompleted`
- From: `Completed`
- On: `PollEventsStrict`(after_cursor, latest_cursor, limit)
- Guards:
  - `cursor_stale`
- Emits: `RejectStrictEventPoll`
- To: `Completed`

### `PollEventsStrictStaleDestroyed`
- From: `Destroyed`
- On: `PollEventsStrict`(after_cursor, latest_cursor, limit)
- Guards:
  - `cursor_stale`
- Emits: `RejectStrictEventPoll`
- To: `Destroyed`

### `AuthorizeMobEventRouterMemberSubscriptionRunning`
- From: `Running`
- On: `AuthorizeMobEventRouterMemberSubscription`(agent_identity, agent_runtime_id, fence_token)
- Guards:
  - `identity_runtime_matches`
  - `runtime_live`
  - `fence_matches`
  - `session_bound`
- Emits: `AuthorizeMobEventRouterMemberSubscription`
- To: `Running`

### `AuthorizeMobEventRouterMemberSubscriptionStopped`
- From: `Stopped`
- On: `AuthorizeMobEventRouterMemberSubscription`(agent_identity, agent_runtime_id, fence_token)
- Guards:
  - `identity_runtime_matches`
  - `runtime_live`
  - `fence_matches`
  - `session_bound`
- Emits: `AuthorizeMobEventRouterMemberSubscription`
- To: `Stopped`

### `AuthorizeMobEventRouterMemberSubscriptionCompleted`
- From: `Completed`
- On: `AuthorizeMobEventRouterMemberSubscription`(agent_identity, agent_runtime_id, fence_token)
- Guards:
  - `identity_runtime_matches`
  - `runtime_live`
  - `fence_matches`
  - `session_bound`
- Emits: `AuthorizeMobEventRouterMemberSubscription`
- To: `Completed`

### `AuthorizeMobEventRouterMemberSubscriptionDestroyed`
- From: `Destroyed`
- On: `AuthorizeMobEventRouterMemberSubscription`(agent_identity, agent_runtime_id, fence_token)
- Guards:
  - `identity_runtime_matches`
  - `runtime_live`
  - `fence_matches`
  - `session_bound`
- Emits: `AuthorizeMobEventRouterMemberSubscription`
- To: `Destroyed`

### `AuthorizeMobEventRouterMemberRemovalMissingRunning`
- From: `Running`
- On: `AuthorizeMobEventRouterMemberRemoval`(agent_identity)
- Guards:
  - `identity_absent`
- Emits: `AuthorizeMobEventRouterMemberRemoval`
- To: `Running`

### `AuthorizeMobEventRouterMemberRemovalMissingStopped`
- From: `Stopped`
- On: `AuthorizeMobEventRouterMemberRemoval`(agent_identity)
- Guards:
  - `identity_absent`
- Emits: `AuthorizeMobEventRouterMemberRemoval`
- To: `Stopped`

### `AuthorizeMobEventRouterMemberRemovalMissingCompleted`
- From: `Completed`
- On: `AuthorizeMobEventRouterMemberRemoval`(agent_identity)
- Guards:
  - `identity_absent`
- Emits: `AuthorizeMobEventRouterMemberRemoval`
- To: `Completed`

### `AuthorizeMobEventRouterMemberRemovalMissingDestroyed`
- From: `Destroyed`
- On: `AuthorizeMobEventRouterMemberRemoval`(agent_identity)
- Guards:
  - `identity_absent`
- Emits: `AuthorizeMobEventRouterMemberRemoval`
- To: `Destroyed`

### `AuthorizeMobEventRouterMemberRemovalUnboundRunning`
- From: `Running`
- On: `AuthorizeMobEventRouterMemberRemoval`(agent_identity)
- Guards:
  - `identity_present`
  - `session_unbound`
- Emits: `AuthorizeMobEventRouterMemberRemoval`
- To: `Running`

### `AuthorizeMobEventRouterMemberRemovalUnboundStopped`
- From: `Stopped`
- On: `AuthorizeMobEventRouterMemberRemoval`(agent_identity)
- Guards:
  - `identity_present`
  - `session_unbound`
- Emits: `AuthorizeMobEventRouterMemberRemoval`
- To: `Stopped`

### `AuthorizeMobEventRouterMemberRemovalUnboundCompleted`
- From: `Completed`
- On: `AuthorizeMobEventRouterMemberRemoval`(agent_identity)
- Guards:
  - `identity_present`
  - `session_unbound`
- Emits: `AuthorizeMobEventRouterMemberRemoval`
- To: `Completed`

### `AuthorizeMobEventRouterMemberRemovalUnboundDestroyed`
- From: `Destroyed`
- On: `AuthorizeMobEventRouterMemberRemoval`(agent_identity)
- Guards:
  - `identity_present`
  - `session_unbound`
- Emits: `AuthorizeMobEventRouterMemberRemoval`
- To: `Destroyed`

### `AuthorizeMobEventRouterMemberRemovalRuntimeNotLiveRunning`
- From: `Running`
- On: `AuthorizeMobEventRouterMemberRemoval`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_not_live`
- Emits: `AuthorizeMobEventRouterMemberRemoval`
- To: `Running`

### `AuthorizeMobEventRouterMemberRemovalRuntimeNotLiveStopped`
- From: `Stopped`
- On: `AuthorizeMobEventRouterMemberRemoval`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_not_live`
- Emits: `AuthorizeMobEventRouterMemberRemoval`
- To: `Stopped`

### `AuthorizeMobEventRouterMemberRemovalRuntimeNotLiveCompleted`
- From: `Completed`
- On: `AuthorizeMobEventRouterMemberRemoval`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_not_live`
- Emits: `AuthorizeMobEventRouterMemberRemoval`
- To: `Completed`

### `AuthorizeMobEventRouterMemberRemovalRuntimeNotLiveDestroyed`
- From: `Destroyed`
- On: `AuthorizeMobEventRouterMemberRemoval`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_not_live`
- Emits: `AuthorizeMobEventRouterMemberRemoval`
- To: `Destroyed`

### `ShutdownRunning`
- From: `Running`
- On: `Shutdown`()
- Emits: `EmitRunLifecycleNotice`
- To: `Stopped`

### `ShutdownStopped`
- From: `Stopped`
- On: `Shutdown`()
- Emits: `EmitRunLifecycleNotice`
- To: `Stopped`

### `ShutdownCompleted`
- From: `Completed`
- On: `Shutdown`()
- Emits: `EmitRunLifecycleNotice`
- To: `Completed`

### `CancelFlowRunning`
- From: `Running`
- On: `CancelFlow`(run_id)
- Guards:
  - `run_known`
- Emits: `NotifyCoordinator`
- To: `Running`

### `InitializeOrchestratorRunning`
- From: `Running`
- On: `InitializeOrchestrator`()
- Emits: `NotifyCoordinator`
- To: `Running`

### `BindCoordinatorRunning`
- From: `Running`
- On: `BindCoordinator`()
- Emits: `NotifyCoordinator`
- To: `Running`

### `UnbindCoordinatorRunning`
- From: `Running`
- On: `UnbindCoordinator`()
- Emits: `NotifyCoordinator`
- To: `Running`

### `StageSpawnRunning`
- From: `Running`
- On: `StageSpawn`(agent_identity, session_id)
- Guards:
  - `pending_identity_unused`
- Emits: `ExposePendingSpawn`, `PendingSpawnOperationOwnerAuthorized`
- To: `Running`

### `StopOrchestratorRunning`
- From: `Running`
- On: `StopOrchestrator`()
- Emits: `NotifyCoordinator`
- To: `Running`

### `StopOrchestratorStopped`
- From: `Stopped`
- On: `StopOrchestrator`()
- Emits: `NotifyCoordinator`
- To: `Stopped`

### `StopOrchestratorCompleted`
- From: `Completed`
- On: `StopOrchestrator`()
- Emits: `NotifyCoordinator`
- To: `Completed`

### `ResumeOrchestratorRunning`
- From: `Running`
- On: `ResumeOrchestrator`()
- Emits: `NotifyCoordinator`
- To: `Running`

### `ResumeOrchestratorStopped`
- From: `Stopped`
- On: `ResumeOrchestrator`()
- Emits: `NotifyCoordinator`
- To: `Stopped`

### `ResumeOrchestratorCompleted`
- From: `Completed`
- On: `ResumeOrchestrator`()
- Emits: `NotifyCoordinator`
- To: `Completed`

### `DestroyOrchestratorRunning`
- From: `Running`
- On: `DestroyOrchestrator`()
- Emits: `NotifyCoordinator`
- To: `Running`

### `DestroyOrchestratorStopped`
- From: `Stopped`
- On: `DestroyOrchestrator`()
- Emits: `NotifyCoordinator`
- To: `Stopped`

### `DestroyOrchestratorCompleted`
- From: `Completed`
- On: `DestroyOrchestrator`()
- Emits: `NotifyCoordinator`
- To: `Completed`

### `ForceCancelMemberRunning`
- From: `Running`
- On: `ForceCancelMember`()
- Emits: `EmitMemberTerminalNotice`
- To: `Running`

### `MemberPeerExposedRunning`
- From: `Running`
- On: `MemberPeerExposed`()
- Emits: `AdmitPeerInput`
- To: `Running`

### `MemberTerminalizedRunning`
- From: `Running`
- On: `MemberTerminalized`()
- Emits: `EmitMemberTerminalNotice`
- To: `Running`

### `OperationPeerTrustedRunning`
- From: `Running`
- On: `OperationPeerTrusted`()
- Emits: `AdmitPeerInput`
- To: `Running`

### `PeerInputAdmittedRunning`
- From: `Running`
- On: `PeerInputAdmitted`()
- Emits: `AdmitPeerInput`
- To: `Running`

### `BeginCleanupStopped`
- From: `Stopped`
- On: `BeginCleanup`()
- Emits: `EmitRunLifecycleNotice`
- To: `Stopped`

### `BeginCleanupCompleted`
- From: `Completed`
- On: `BeginCleanup`()
- Emits: `EmitRunLifecycleNotice`
- To: `Stopped`

### `FinishCleanupStopped`
- From: `Stopped`
- On: `FinishCleanup`()
- Emits: `EmitRunLifecycleNotice`
- To: `Stopped`

### `FinishCleanupCompleted`
- From: `Completed`
- On: `FinishCleanup`()
- Emits: `EmitRunLifecycleNotice`
- To: `Stopped`

### `RunFlowRunning`
- From: `Running`
- On: `RunFlow`(run_id, step_ids, ordered_steps, step_status, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, step_target_terminal_failure_counts, escalation_threshold, max_step_retries, max_active_nodes, max_active_frames, max_frame_depth)
- Guards:
  - `coordinator_bound`
  - `run_seed_is_new`
  - `run_seed_ordered_steps_match_tracked_steps`
  - `run_seed_defaults_cover_tracked_steps`
  - `run_seed_maps_cover_tracked_steps`
  - `run_seed_map_keys_are_tracked`
  - `run_seed_dependency_targets_are_tracked`
- Emits: `EmitFlowRunNotice`
- To: `Running`

### `CreateRunSeedRunning`
- From: `Running`
- On: `CreateRunSeed`(run_id, step_ids, ordered_steps, step_status, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, step_target_terminal_failure_counts, escalation_threshold, max_step_retries, max_active_nodes, max_active_frames, max_frame_depth)
- Guards:
  - `run_seed_is_new`
  - `run_seed_ordered_steps_match_tracked_steps`
  - `run_seed_defaults_cover_tracked_steps`
  - `run_seed_maps_cover_tracked_steps`
  - `run_seed_map_keys_are_tracked`
  - `run_seed_dependency_targets_are_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `CreateFrameSeedRunning`
- From: `Running`
- On: `CreateFrameSeed`(run_id, frame_id, frame_scope, loop_instance_id, iteration, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, node_step_ids, node_loop_ids, node_status, ready_queue, output_recorded, node_condition_results, last_admitted_node)
- Guards:
  - `frame_seed_is_new`
  - `run_known`
  - `body_frame_has_parent_loop`
  - `frame_seed_has_no_last_admitted_node`
  - `frame_seed_ordered_nodes_match_tracked_nodes`
  - `frame_seed_status_covers_tracked_nodes`
  - `frame_seed_defaults_cover_tracked_nodes`
  - `frame_seed_maps_cover_tracked_nodes`
  - `frame_seed_map_keys_are_tracked`
  - `frame_seed_ready_queue_nodes_are_tracked`
  - `frame_seed_dependency_targets_are_tracked`
  - `frame_seed_node_kind_covers_tracked_nodes`
  - `frame_seed_node_kind_keys_are_tracked`
  - `frame_seed_ready_queue_matches_dependency_roots`
  - `frame_seed_step_nodes_have_exact_step_ids`
  - `frame_seed_loop_nodes_have_exact_loop_ids`
  - `frame_seed_step_id_keys_are_tracked_steps`
  - `frame_seed_loop_id_keys_are_tracked_loops`
- Emits: `EmitRunLifecycleNotice`, `FrameSeedConfirmed`
- To: `Running`

### `CreateFrameSeedAlreadySeededRunning`
- From: `Running`
- On: `CreateFrameSeed`(run_id, frame_id, frame_scope, loop_instance_id, iteration, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, node_step_ids, node_loop_ids, node_status, ready_queue, output_recorded, node_condition_results, last_admitted_node)
- Guards:
  - `frame_seed_already_tracked`
- Emits: `FrameSeedConfirmed`
- To: `Running`

### `CreateFrameSeedAlreadySeededStopped`
- From: `Stopped`
- On: `CreateFrameSeed`(run_id, frame_id, frame_scope, loop_instance_id, iteration, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, node_step_ids, node_loop_ids, node_status, ready_queue, output_recorded, node_condition_results, last_admitted_node)
- Guards:
  - `frame_seed_already_tracked`
- Emits: `FrameSeedConfirmed`
- To: `Stopped`

### `CreateFrameSeedAlreadySeededCompleted`
- From: `Completed`
- On: `CreateFrameSeed`(run_id, frame_id, frame_scope, loop_instance_id, iteration, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, node_step_ids, node_loop_ids, node_status, ready_queue, output_recorded, node_condition_results, last_admitted_node)
- Guards:
  - `frame_seed_already_tracked`
- Emits: `FrameSeedConfirmed`
- To: `Completed`

### `CreateFrameSeedAlreadySeededDestroyed`
- From: `Destroyed`
- On: `CreateFrameSeed`(run_id, frame_id, frame_scope, loop_instance_id, iteration, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, node_step_ids, node_loop_ids, node_status, ready_queue, output_recorded, node_condition_results, last_admitted_node)
- Guards:
  - `frame_seed_already_tracked`
- Emits: `FrameSeedConfirmed`
- To: `Destroyed`

### `CreateLoopSeedRunning`
- From: `Running`
- On: `CreateLoopSeed`(loop_instance_id, parent_frame_id, parent_node_id, loop_id, depth, max_iterations)
- Guards:
  - `loop_seed_is_new`
  - `parent_frame_known`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `RecordLoopBodyFrameCompletedRunning`
- From: `Running`, `Stopped`, `Completed`
- On: `RecordLoopBodyFrameCompleted`(loop_instance_id, iteration)
- Guards:
  - `known_loop`
  - `loop_running`
  - `body_frame_active`
  - `iteration_matches_current`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `RecordLoopUntilConditionMetRunning`
- From: `Running`, `Stopped`, `Completed`
- On: `RecordLoopUntilConditionMet`(loop_instance_id, iteration)
- Guards:
  - `known_loop`
  - `loop_running`
  - `awaiting_until_evaluation`
  - `iteration_matches_last_completed`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `RecordLoopUntilConditionFailedRunning`
- From: `Running`, `Stopped`, `Completed`
- On: `RecordLoopUntilConditionFailed`(loop_instance_id, iteration)
- Guards:
  - `known_loop`
  - `loop_running`
  - `awaiting_until_evaluation`
  - `iteration_matches_last_completed`
  - `iterations_remaining`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `RecordLoopUntilConditionFailedExhausted`
- From: `Running`, `Stopped`, `Completed`
- On: `RecordLoopUntilConditionFailed`(loop_instance_id, iteration)
- Guards:
  - `known_loop`
  - `loop_running`
  - `awaiting_until_evaluation`
  - `iteration_matches_last_completed`
  - `iterations_exhausted`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandStartRun`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `start_run_command`
  - `run_pending`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandDispatchStep`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `dispatch_step_command`
  - `has_step_id`
  - `step_tracked`
  - `dispatched_step_status`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandCompleteStep`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `complete_step_command`
  - `has_step_id`
  - `completed_step_status`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandRecordStepOutput`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `record_step_output_command`
  - `has_step_id`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandConditionPassed`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `condition_passed_command`
  - `has_step_id`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandConditionRejected`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `condition_rejected_command`
  - `has_step_id`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandFailStep`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `fail_step_command`
  - `has_step_id`
  - `failed_step_status`
  - `step_tracked`
  - `supervisor_escalation_not_due`
- Emits: `EmitRunLifecycleNotice`, `AppendFailureLedger`
- To: `Running`

### `AuthorizeFlowRunReducerCommandFailStepEscalating`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `fail_step_command`
  - `has_step_id`
  - `failed_step_status`
  - `step_tracked`
  - `supervisor_escalation_due`
- Emits: `EmitRunLifecycleNotice`, `AppendFailureLedger`, `EscalateSupervisor`
- To: `Running`

### `AuthorizeFlowRunReducerCommandSkipStep`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `skip_step_command`
  - `has_step_id`
  - `skipped_step_status`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandProjectFrameStepStatus`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `project_frame_step_status_command`
  - `has_step_id`
  - `has_frame_id`
  - `has_node_id`
  - `step_tracked`
  - `frame_belongs_to_run`
  - `frame_node_tracked`
  - `frame_node_maps_to_step`
  - `run_step_not_already_terminal_projected`
  - `frame_node_completed_or_skipped`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandProjectFrameStepStatusFailed`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `project_frame_step_status_command`
  - `has_step_id`
  - `has_frame_id`
  - `has_node_id`
  - `step_tracked`
  - `frame_belongs_to_run`
  - `frame_node_tracked`
  - `frame_node_maps_to_step`
  - `run_step_not_already_terminal_projected`
  - `frame_node_failed`
  - `supervisor_escalation_not_due`
- Emits: `EmitRunLifecycleNotice`, `AppendFailureLedger`
- To: `Running`

### `AuthorizeFlowRunReducerCommandProjectFrameStepStatusFailedEscalating`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `project_frame_step_status_command`
  - `has_step_id`
  - `has_frame_id`
  - `has_node_id`
  - `step_tracked`
  - `frame_belongs_to_run`
  - `frame_node_tracked`
  - `frame_node_maps_to_step`
  - `run_step_not_already_terminal_projected`
  - `frame_node_failed`
  - `supervisor_escalation_due`
- Emits: `EmitRunLifecycleNotice`, `AppendFailureLedger`, `EscalateSupervisor`
- To: `Running`

### `AuthorizeFlowRunReducerCommandCancelStep`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `cancel_step_command`
  - `has_step_id`
  - `step_tracked`
  - `canceled_step_status`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandRegisterTargets`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `register_targets_command`
  - `has_step_id`
  - `has_target_count`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandRecordTargetSuccess`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `record_target_success_command`
  - `has_step_id`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandRecordTargetTerminalFailure`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `record_target_terminal_failure_command`
  - `has_step_id`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandRecordTargetCanceled`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `record_target_canceled_command`
  - `has_step_id`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandRecordTargetFailure`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `record_target_failure_command`
  - `has_step_id`
  - `has_retry_key`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandRegisterReadyFrame`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `register_ready_frame_command`
  - `has_frame_id`
  - `known_frame`
  - `frame_not_already_ready`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandRegisterReadyFrameAlreadyReady`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `register_ready_frame_command`
  - `has_frame_id`
  - `known_frame`
  - `frame_already_ready`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandPumpNodeScheduler`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `pump_node_scheduler_command`
  - `has_frame_id`
  - `ready_frame_registered`
  - `machine_selected_ready_frame`
  - `node_capacity_available`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandRegisterPendingBodyFrame`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `register_pending_body_frame_command`
  - `has_loop_instance_id`
  - `known_loop`
  - `loop_not_already_pending_body_frame`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandPumpFrameScheduler`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `pump_frame_scheduler_command`
  - `has_loop_instance_id`
  - `pending_body_frame_registered`
  - `machine_selected_pending_body_frame_loop`
  - `frame_capacity_available`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandNodeExecutionReleased`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `node_execution_released_command`
  - `active_node_count_present`
  - `active_node_count_positive`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandFrameTerminated`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `frame_terminated_command`
  - `active_frame_count_present`
  - `active_frame_count_positive`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandFrameTerminatedNoActiveFrame`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `frame_terminated_command`
  - `active_frame_count_present`
  - `active_frame_count_zero`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandTerminalCompleted`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `terminal_completed_command`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandTerminalFailed`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `terminal_failed_command`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandTerminalCanceled`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `terminal_canceled_command`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowFrameReducerCommandAdmitNextReadyNode`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowFrameReducerCommand`(frame_id, command, node_id, node_status, terminal_status)
- Guards:
  - `known_frame`
  - `frame_running`
  - `admit_next_ready_node_command`
  - `no_terminal_status`
  - `has_node_id`
  - `running_node_status`
  - `node_tracked`
  - `node_currently_ready`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowFrameReducerCommandCompleteNode`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowFrameReducerCommand`(frame_id, command, node_id, node_status, terminal_status)
- Guards:
  - `known_frame`
  - `frame_running`
  - `complete_node_command`
  - `no_terminal_status`
  - `has_node_id`
  - `completed_node_status`
  - `node_tracked`
  - `node_currently_running`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowFrameReducerCommandRecordNodeOutput`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowFrameReducerCommand`(frame_id, command, node_id, node_status, terminal_status)
- Guards:
  - `known_frame`
  - `frame_running`
  - `record_node_output_command`
  - `no_terminal_status`
  - `has_node_id`
  - `node_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowFrameReducerCommandFailNode`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowFrameReducerCommand`(frame_id, command, node_id, node_status, terminal_status)
- Guards:
  - `known_frame`
  - `frame_running`
  - `fail_node_command`
  - `no_terminal_status`
  - `has_node_id`
  - `failed_node_status`
  - `node_tracked`
  - `node_currently_running`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowFrameReducerCommandSkipNode`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowFrameReducerCommand`(frame_id, command, node_id, node_status, terminal_status)
- Guards:
  - `known_frame`
  - `frame_running`
  - `skip_node_command`
  - `no_terminal_status`
  - `has_node_id`
  - `skipped_node_status`
  - `node_tracked`
  - `node_currently_running`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowFrameReducerCommandCancelNode`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowFrameReducerCommand`(frame_id, command, node_id, node_status, terminal_status)
- Guards:
  - `known_frame`
  - `frame_running`
  - `cancel_node_command`
  - `no_terminal_status`
  - `has_node_id`
  - `canceled_node_status`
  - `node_tracked`
  - `node_currently_running`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowFrameReducerCommandSealFrameRunning`
- From: `Running`
- On: `AuthorizeFlowFrameReducerCommand`(frame_id, command, node_id, node_status, terminal_status)
- Guards:
  - `known_frame`
  - `frame_running`
  - `seal_frame_command`
  - `terminal_frame_status`
  - `all_nodes_terminal`
  - `terminal_class_matches_nodes`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowFrameReducerCommandSealFrameStopped`
- From: `Stopped`
- On: `AuthorizeFlowFrameReducerCommand`(frame_id, command, node_id, node_status, terminal_status)
- Guards:
  - `known_frame`
  - `frame_running`
  - `seal_frame_command`
  - `terminal_frame_status`
  - `all_nodes_terminal`
  - `terminal_class_matches_nodes`
- Emits: `EmitRunLifecycleNotice`
- To: `Stopped`

### `AuthorizeFlowFrameReducerCommandSealFrameCompleted`
- From: `Completed`
- On: `AuthorizeFlowFrameReducerCommand`(frame_id, command, node_id, node_status, terminal_status)
- Guards:
  - `known_frame`
  - `frame_running`
  - `seal_frame_command`
  - `terminal_frame_status`
  - `all_nodes_terminal`
  - `terminal_class_matches_nodes`
- Emits: `EmitRunLifecycleNotice`
- To: `Completed`

### `AuthorizeLoopIterationReducerCommandBodyFrameStarted`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeLoopIterationReducerCommand`(loop_instance_id, command, body_frame_id, body_frame_iteration)
- Guards:
  - `known_loop`
  - `loop_running`
  - `body_frame_started_command`
  - `blocked_use_CreateFrameSeed_body_side_effect`
  - `no_body_frame_iteration`
  - `body_frame_already_active`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeLoopIterationReducerCommandBodyFrameCompleted`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeLoopIterationReducerCommand`(loop_instance_id, command, body_frame_id, body_frame_iteration)
- Guards:
  - `known_loop`
  - `loop_running`
  - `body_frame_active`
  - `body_frame_completed_command`
  - `blocked_use_RecordLoopBodyFrameCompleted`
  - `body_frame_iteration_present`
  - `iteration_matches_current`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeLoopIterationReducerCommandBodyFrameFailed`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeLoopIterationReducerCommand`(loop_instance_id, command, body_frame_id, body_frame_iteration)
- Guards:
  - `known_loop`
  - `loop_running`
  - `body_frame_active`
  - `body_frame_failed_command`
  - `body_frame_iteration_present`
  - `iteration_matches_current`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeLoopIterationReducerCommandBodyFrameCanceled`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeLoopIterationReducerCommand`(loop_instance_id, command, body_frame_id, body_frame_iteration)
- Guards:
  - `known_loop`
  - `loop_running`
  - `body_frame_active`
  - `body_frame_canceled_command`
  - `body_frame_iteration_present`
  - `iteration_matches_current`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeLoopIterationReducerCommandUntilFeedback`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeLoopIterationReducerCommand`(loop_instance_id, command, body_frame_id, body_frame_iteration)
- Guards:
  - `known_loop`
  - `loop_running`
  - `awaiting_until_evaluation`
  - `blocked_use_RecordLoopUntilConditionFeedback`
  - `no_body_frame_iteration`
  - `until_feedback_command`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeLoopIterationReducerCommandCancelLoop`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeLoopIterationReducerCommand`(loop_instance_id, command, body_frame_id, body_frame_iteration)
- Guards:
  - `known_loop`
  - `loop_running`
  - `cancel_loop_command`
  - `no_body_frame_iteration`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `StartFlowRunning`
- From: `Running`
- On: `StartFlow`()
- Guards:
  - `coordinator_bound`
- Emits: `EmitFlowRunNotice`
- To: `Running`

### `CreateRunRunning`
- From: `Running`
- On: `CreateRun`()
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `StartRunRunning`
- From: `Running`
- On: `StartRun`()
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `CompleteFlowRunning`
- From: `Running`, `Completed`
- On: `CompleteFlow`()
- Guards:
  - `active_runs_present`
- Emits: `FlowTerminalized`
- To: `Running`

### `CompleteFlowRunningZero`
- From: `Running`, `Completed`
- On: `CompleteFlow`()
- Guards:
  - `no_active_runs`
- Emits: `NotifyCoordinator`
- To: `Running`

### `FinishRunRunning`
- From: `Running`, `Stopped`
- On: `FinishRun`()
- Guards:
  - `active_runs_present`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `FinishRunRunningZero`
- From: `Running`, `Stopped`
- On: `FinishRun`()
- Guards:
  - `no_active_runs`
- Emits: `NotifyCoordinator`
- To: `Running`

### `RetireRunningReleasing`
- From: `Running`
- On: `Retire`(mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id)
- Guards:
  - `active_members_present`
  - `runtime_id_present`
  - `identity_binding_matches`
  - `generation_matches`
  - `prior_session_binding_present`
  - `releasing_present`
  - `releasing_matches_current`
  - `session_matches_releasing`
- Emits: `AppendLifecycleJournal`, `RequestRuntimeRetire`, `RequestSessionIngressDetachForMobDestroy`, `MemberSessionBindingChanged`, `RequestKickoffQuiesce`
- To: `Running`

### `RetireRunningPreservingBinding`
- From: `Running`
- On: `Retire`(mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id)
- Guards:
  - `active_members_present`
  - `runtime_id_present`
  - `identity_binding_matches`
  - `generation_matches`
  - `prior_session_binding_present`
  - `releasing_absent`
  - `session_binding_matches`
- Emits: `AppendLifecycleJournal`, `RequestRuntimeRetire`, `RequestKickoffQuiesce`
- To: `Running`

### `RetireRunningNoBinding`
- From: `Running`
- On: `Retire`(mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id)
- Guards:
  - `active_members_present`
  - `runtime_id_present`
  - `identity_binding_matches`
  - `generation_matches`
  - `no_prior_session_binding`
  - `releasing_absent`
  - `session_absent`
- Emits: `AppendLifecycleJournal`, `RequestKickoffQuiesce`
- To: `Running`

### `RetireStoppedReleasing`
- From: `Stopped`
- On: `Retire`(mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id)
- Guards:
  - `active_members_present`
  - `runtime_id_present`
  - `identity_binding_matches`
  - `generation_matches`
  - `prior_session_binding_present`
  - `releasing_present`
  - `releasing_matches_current`
  - `session_matches_releasing`
- Emits: `AppendLifecycleJournal`, `RequestRuntimeRetire`, `RequestSessionIngressDetachForMobDestroy`, `MemberSessionBindingChanged`, `RequestKickoffQuiesce`
- To: `Stopped`

### `RequestPendingSessionIngressDetachForMobDestroyRunning`
- From: `Running`
- On: `RequestPendingSessionIngressDetachForMobDestroy`(mob_id, agent_runtime_id)
- Guards:
  - `pending_detach_present`
- Emits: `RequestSessionIngressDetachForMobDestroy`
- To: `Running`

### `RequestPendingSessionIngressDetachForMobDestroyStopped`
- From: `Stopped`
- On: `RequestPendingSessionIngressDetachForMobDestroy`(mob_id, agent_runtime_id)
- Guards:
  - `pending_detach_present`
- Emits: `RequestSessionIngressDetachForMobDestroy`
- To: `Stopped`

### `SessionIngressDetachedForMobDestroyRunning`
- From: `Running`
- On: `SessionIngressDetachedForMobDestroy`(mob_id, agent_runtime_id)
- Guards:
  - `pending_detach_present`
- To: `Running`

### `SessionIngressDetachedForMobDestroyStopped`
- From: `Stopped`
- On: `SessionIngressDetachedForMobDestroy`(mob_id, agent_runtime_id)
- Guards:
  - `pending_detach_present`
- To: `Stopped`

### `SessionIngressDetachFailedForMobDestroyRunning`
- From: `Running`
- On: `SessionIngressDetachFailedForMobDestroy`(mob_id, agent_runtime_id, reason)
- Guards:
  - `pending_detach_present`
- To: `Running`

### `SessionIngressDetachFailedForMobDestroyStopped`
- From: `Stopped`
- On: `SessionIngressDetachFailedForMobDestroy`(mob_id, agent_runtime_id, reason)
- Guards:
  - `pending_detach_present`
- To: `Stopped`

### `RetireStoppedPreservingBinding`
- From: `Stopped`
- On: `Retire`(mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id)
- Guards:
  - `active_members_present`
  - `runtime_id_present`
  - `identity_binding_matches`
  - `generation_matches`
  - `prior_session_binding_present`
  - `releasing_absent`
  - `session_binding_matches`
- Emits: `AppendLifecycleJournal`, `RequestRuntimeRetire`, `RequestKickoffQuiesce`
- To: `Stopped`

### `RetireStoppedNoBinding`
- From: `Stopped`
- On: `Retire`(mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id)
- Guards:
  - `active_members_present`
  - `runtime_id_present`
  - `identity_binding_matches`
  - `generation_matches`
  - `no_prior_session_binding`
  - `releasing_absent`
  - `session_absent`
- Emits: `AppendLifecycleJournal`, `RequestKickoffQuiesce`
- To: `Stopped`

### `RetireAbsentRunning`
- From: `Running`
- On: `RetireAbsent`(agent_identity)
- Guards:
  - `identity_absent`
- To: `Running`

### `RetireAbsentStopped`
- From: `Stopped`
- On: `RetireAbsent`(agent_identity)
- Guards:
  - `identity_absent`
- To: `Stopped`

### `RetireAllRunning`
- From: `Running`
- On: `RetireAll`()
- Emits: `EmitMemberLifecycleNotice`
- To: `Running`

### `RetireAllStopped`
- From: `Stopped`
- On: `RetireAll`()
- Emits: `EmitMemberLifecycleNotice`
- To: `Stopped`

### `RetireAllCompleted`
- From: `Completed`
- On: `RetireAll`()
- Emits: `EmitMemberLifecycleNotice`
- To: `Completed`

### `CompleteSpawnRunning`
- From: `Running`, `Stopped`
- On: `CompleteSpawn`(agent_identity)
- Guards:
  - `pending_spawns_present`
  - `pending_identity_present`
- Emits: `EmitMemberLifecycleNotice`
- To: `Running`

### `CompleteSpawnLateArrivalRunning`
- From: `Running`
- On: `CompleteSpawn`(agent_identity)
- Guards:
  - `pending_identity_absent`
- To: `Running`

### `CompleteSpawnLateArrivalStopped`
- From: `Stopped`
- On: `CompleteSpawn`(agent_identity)
- Guards:
  - `pending_identity_absent`
- To: `Stopped`

### `CompleteSpawnLateArrivalCompleted`
- From: `Completed`
- On: `CompleteSpawn`(agent_identity)
- Guards:
  - `pending_identity_absent`
- To: `Completed`

### `CompleteSpawnDestroyed`
- From: `Destroyed`
- On: `CompleteSpawn`(agent_identity)
- To: `Destroyed`

### `CancelPendingSpawnPresentRunning`
- From: `Running`
- On: `CancelPendingSpawn`(agent_identity)
- Guards:
  - `pending_spawns_present`
  - `pending_identity_present`
- To: `Running`

### `CancelPendingSpawnPresentStopped`
- From: `Stopped`
- On: `CancelPendingSpawn`(agent_identity)
- Guards:
  - `pending_spawns_present`
  - `pending_identity_present`
- To: `Stopped`

### `CancelPendingSpawnPresentCompleted`
- From: `Completed`
- On: `CancelPendingSpawn`(agent_identity)
- Guards:
  - `pending_spawns_present`
  - `pending_identity_present`
- To: `Completed`

### `CancelPendingSpawnAbsentRunning`
- From: `Running`
- On: `CancelPendingSpawn`(agent_identity)
- Guards:
  - `pending_identity_absent`
- To: `Running`

### `CancelPendingSpawnAbsentStopped`
- From: `Stopped`
- On: `CancelPendingSpawn`(agent_identity)
- Guards:
  - `pending_identity_absent`
- To: `Stopped`

### `CancelPendingSpawnAbsentCompleted`
- From: `Completed`
- On: `CancelPendingSpawn`(agent_identity)
- Guards:
  - `pending_identity_absent`
- To: `Completed`

### `CancelPendingSpawnDestroyed`
- From: `Destroyed`
- On: `CancelPendingSpawn`(agent_identity)
- To: `Destroyed`

### `DestroyFromAny`
- From: `Running`, `Stopped`, `Completed`
- On: `Destroy`()
- Guards:
  - `session_ingress_detaches_closed`
  - `kickoff_waiters_quiesced`
  - `pending_spawns_drained`
- To: `Destroyed`

### `RespawnRunning`
- From: `Running`
- On: `Respawn`(agent_runtime_id)
- Guards:
  - `runtime_id_present`
  - `coordinator_bound`
- Emits: `ExposePendingSpawn`
- To: `Running`

### `CancelAllWorkRunning`
- From: `Running`
- On: `CancelAllWork`(agent_identity, agent_runtime_id, fence_token)
- Guards:
  - `active_members_present`
  - `identity_binding_matches`
  - `current_binding_matches`
  - `fence_token_matches`
- Emits: `FlowTerminalized`
- To: `Running`

### `ResolveCancelAllWorkRejectionStopped`
- From: `Stopped`
- On: `ResolveCancelAllWorkRejection`(agent_identity, agent_runtime_id, fence_token)
- Emits: `CancelAllWorkRejected`
- To: `Stopped`

### `ResolveCancelAllWorkRejectionCompleted`
- From: `Completed`
- On: `ResolveCancelAllWorkRejection`(agent_identity, agent_runtime_id, fence_token)
- Emits: `CancelAllWorkRejected`
- To: `Completed`

### `ResolveCancelAllWorkRejectionDestroyed`
- From: `Destroyed`
- On: `ResolveCancelAllWorkRejection`(agent_identity, agent_runtime_id, fence_token)
- Emits: `CancelAllWorkRejected`
- To: `Destroyed`

### `ResolveCancelAllWorkRejectionMemberNotFound`
- From: `Running`
- On: `ResolveCancelAllWorkRejection`(agent_identity, agent_runtime_id, fence_token)
- Guards:
  - `identity_absent`
- Emits: `CancelAllWorkRejected`
- To: `Running`

### `ResolveCancelAllWorkRejectionCurrentRuntimeNotLive`
- From: `Running`
- On: `ResolveCancelAllWorkRejection`(agent_identity, agent_runtime_id, fence_token)
- Guards:
  - `identity_present`
  - `current_runtime_not_live`
- Emits: `CancelAllWorkRejected`
- To: `Running`

### `ResolveCancelAllWorkRejectionStaleFenceToken`
- From: `Running`
- On: `ResolveCancelAllWorkRejection`(agent_identity, agent_runtime_id, fence_token)
- Guards:
  - `identity_present`
  - `current_runtime_live`
  - `runtime_or_fence_stale`
- Emits: `CancelAllWorkRejected`
- To: `Running`

### `RecordCoordinationWorkIntent`
- From: `Running`
- On: `RecordCoordinationWorkIntent`(intent_id, requested_status, owner_present, summary_present, metadata_public, draft_mob_id, authority_mob_id, resource_tokens, expires_at_ms)
- Guards:
  - `intent_is_new`
  - `summary_present`
  - `metadata_public`
  - `owning_mob_ref_matches`
  - `resources_non_empty`
  - `owner_present`
- Emits: `WorkIntentRecorded`
- To: `Running`

### `RecordCoordinationResourceClaim`
- From: `Running`
- On: `RecordCoordinationResourceClaim`(claim_id, requested_kind, requested_status, owner_present, metadata_public, draft_mob_id, authority_mob_id, resource_tokens, expires_at_ms)
- Guards:
  - `claim_is_new`
  - `metadata_public`
  - `owning_mob_ref_matches`
  - `resources_non_empty`
  - `owner_present`
- Emits: `ResourceClaimRecorded`
- To: `Running`

### `UpdateCoordinationWorkIntentPlanned`
- From: `Running`
- On: `UpdateCoordinationWorkIntentStatus`(intent_id, expected_revision, requested_status, now_ms)
- Guards:
  - `intent_present`
  - `revision_cas`
  - `target_is_planned`
  - `not_expired`
- Emits: `WorkIntentStatusChanged`
- To: `Running`

### `UpdateCoordinationWorkIntentActive`
- From: `Running`
- On: `UpdateCoordinationWorkIntentStatus`(intent_id, expected_revision, requested_status, now_ms)
- Guards:
  - `intent_present`
  - `revision_cas`
  - `target_is_active`
  - `not_expired`
- Emits: `WorkIntentStatusChanged`
- To: `Running`

### `UpdateCoordinationWorkIntentBlocked`
- From: `Running`
- On: `UpdateCoordinationWorkIntentStatus`(intent_id, expected_revision, requested_status, now_ms)
- Guards:
  - `intent_present`
  - `revision_cas`
  - `target_is_blocked`
  - `not_expired`
- Emits: `WorkIntentStatusChanged`
- To: `Running`

### `UpdateCoordinationWorkIntentCompleted`
- From: `Running`
- On: `UpdateCoordinationWorkIntentStatus`(intent_id, expected_revision, requested_status, now_ms)
- Guards:
  - `intent_present`
  - `revision_cas`
  - `target_is_completed`
- Emits: `WorkIntentStatusChanged`
- To: `Running`

### `UpdateCoordinationWorkIntentCancelled`
- From: `Running`
- On: `UpdateCoordinationWorkIntentStatus`(intent_id, expected_revision, requested_status, now_ms)
- Guards:
  - `intent_present`
  - `revision_cas`
  - `target_is_cancelled`
- Emits: `WorkIntentStatusChanged`
- To: `Running`

### `UpdateCoordinationResourceClaimActive`
- From: `Running`
- On: `UpdateCoordinationResourceClaimStatus`(claim_id, expected_revision, requested_status, now_ms)
- Guards:
  - `claim_present`
  - `revision_cas`
  - `target_is_active`
  - `not_expired`
- Emits: `ResourceClaimStatusChanged`
- To: `Running`

### `UpdateCoordinationResourceClaimReleased`
- From: `Running`
- On: `UpdateCoordinationResourceClaimStatus`(claim_id, expected_revision, requested_status, now_ms)
- Guards:
  - `claim_present`
  - `revision_cas`
  - `target_is_released`
- Emits: `ResourceClaimStatusChanged`
- To: `Running`

### `UpdateCoordinationResourceClaimExpired`
- From: `Running`
- On: `UpdateCoordinationResourceClaimStatus`(claim_id, expected_revision, requested_status, now_ms)
- Guards:
  - `claim_present`
  - `revision_cas`
  - `target_is_expired`
- Emits: `ResourceClaimStatusChanged`
- To: `Running`

### `UpdateCoordinationResourceClaimCancelled`
- From: `Running`
- On: `UpdateCoordinationResourceClaimStatus`(claim_id, expected_revision, requested_status, now_ms)
- Guards:
  - `claim_present`
  - `revision_cas`
  - `target_is_cancelled`
- Emits: `ResourceClaimStatusChanged`
- To: `Running`

### `ObserveCoordinationResourceClaimOverlap`
- From: `Running`
- On: `ObserveCoordinationResourceClaimOverlap`(claim_id, now_ms, candidate_overlap_ids)
- Guards:
  - `claim_present`
  - `candidates_are_valid_overlaps`
  - `no_omitted_overlap`
- Emits: `ResourceClaimOverlapObserved`
- To: `Running`

## Coverage
### Code Anchors
- `mob_handle_surface` (machine `MobMachine`): `meerkat-mob/src/runtime/handle.rs` — identity-first public MobMachine handle surface for ensure member, reconcile, and member command routing
- `mob_actor_authority` (machine `MobMachine`): `meerkat-mob/src/runtime/actor.rs` — MobMachine actor authority and command execution for wire, unwire, spawn, ensure member, reconcile, observe runtime, submit work, retire, reset, respawn, complete, mark completed, stop/stopped, resume, force cancel, subscribe events, shutdown, destroy, terminalized member, record operator action provenance, flow, run, create frame seed, create loop seed, project frame phase, project loop state, orchestrator, coordinator, cleanup, append failure ledger, escalate supervisor, peer, progress, notices, kickoff resolve started/callback pending/failed/clear, wiring graph, and session binding
- `mob_owner_bridge_cleanup_authority` (machine `MobMachine`): `meerkat-mob-mcp/src/lib.rs` — MobMachine owner bridge session cleanup authority for owner bridge cleanup requires owner and implicit delegation requires owner invariants
- `mob_coordination_board_authority` (machine `MobMachine`): `meerkat-mob/src/coordination.rs` — MobMachine coordination board authority: record work intent, record resource claim, update coordination work intent status planned active blocked completed cancelled, update coordination resource claim status active released expired cancelled, observe coordination resource claim overlap, and the recorded/status-changed/overlap-observed coordination effects
- `mob_operator_admission_authority` (machine `MobMachine`): `meerkat-mob-mcp/src/agent_tools.rs` — MobMachine operator-admission authority for the mob tool surface: resolve create mob admission from the create-mobs capability observation and resolve profile mutation admission from the mutate-profiles capability observation, emitting the create-mob and profile-mutation admission resolved verdicts the surface mirrors (denied -> access denied)
- `mob_membership_classifier_authority` (machine `MobMachine`): `meerkat-mob/src/runtime/actor.rs` — MobMachine membership and runtime-incarnation classifiers owned by the actor: probe member admission duplicate or admitted from machine-owned binding and pending-spawn state; compute respawn generation successor; reconcile desired members to spawn retain or retire against current bindings emitting member spawn required, member retain required, and member retire required; set and observe external member rebind capability available or unavailable; classify turn timeout disposition detached canceled or retryable; and seed orphan budget once at startup, emitting the member admission probed, respawn generation computed, external member rebind capability, and turn timeout disposition classified effects
- `mob_flow_fault_topology_escalation_authority` (machine `MobMachine`): `meerkat-mob/src/runtime/flow.rs` — MobMachine flow-step fault, topology-edge, and supervisor-escalation classifiers owned by the flow engine: classify step output fault retry or terminal malformed json into a step fault disposition; evaluate topology edge rule allow deny or default into a policy decision verdict; and escalate to supervisor target found with a real supervisor identity or no eligible target, emitting the step output fault classified, topology edge verdict resolved, supervisor escalation requested, and supervisor escalation failed effects

### Scenarios
- `coordination-board-records-and-overlap` — record coordination work intent and resource claim, update coordination work intent and resource claim status across planned active blocked completed cancelled released expired, and observe coordination resource claim overlap with recomputed revision and event sequence
- `spawn-work-terminal` — member spawn, ensure member, reconcile, runtime-ready observation, work submission, and terminal work closure
- `retire-respawn-destroy` — member retires, resets, respawns with a new runtime incarnation, stops/stopped, resumes, shuts down, destroys cleanly, and resets to running when reusable
- `wiring-and-session-binding` — wire and unwire members, enforce known identity for session bindings, expose pending spawn, member session binding changed, and wiring lifecycle notices
- `flow-and-run-lifecycle` — run flow, start flow, create run, create frame seed, create loop seed, project frame phase, project loop state, start run, complete flow, finish run, mark completed, kickoff resolve started or failed, kickoff clear, flow terminalized, and force cancel running work
- `event-subscriptions-and-notices` — subscribe agent, all agent, and mob events; emit member, run, flow, progress, terminal, and wiring notices
- `orchestrator-coordinator-cleanup` — initialize, stop, resume, and destroy orchestrator; bind or unbind coordinator; begin and finish cleanup; notify coordinator and escalate supervisor
- `owner-bridge-cleanup` — bind owner bridge session, owner bridge cleanup requires owner, implicit delegation requires owner, and recover owner bridge session authority for archive cleanup
- `operator-provenance-and-peer-input` — record operator action provenance, trust operation peer, admit peer input, append failure ledger, surface peer-exposed member inputs, and resolve operator create mob admission and profile mutation admission verdicts the tool surface mirrors
- `membership-admission-respawn-reconcile-rebind-timeout` — probe member admission duplicate or admitted, compute respawn generation, reconcile desired members to spawn retain or retire emitting member spawn required member retain required and member retire required, set and observe external member rebind capability available or unavailable, classify turn timeout disposition detached canceled or retryable, and seed orphan budget
- `flow-fault-topology-supervisor-escalation` — classify step output fault retry or terminal malformed json into a step fault disposition, evaluate topology edge rule allow deny or default into a policy decision verdict resolved, and escalate to supervisor target found with eligible supervisor identity or no eligible target emitting supervisor escalation requested or failed
