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
- `member_kickoff_objective_ids`: `Map<AgentIdentity, String>`
- `member_kickoff_input_ids`: `Map<AgentIdentity, InputId>`
- `retained_placed_kickoff_obligations`: `Map<AgentIdentity, PlacedKickoffObligation>`
- `member_placed_kickoff_outcome_kinds`: `Map<AgentIdentity, PlacedKickoffOutcomeKind>`
- `member_placed_kickoff_outcome_errors`: `Map<AgentIdentity, String>`
- `member_placed_kickoff_closure_kinds`: `Map<AgentIdentity, PlacedKickoffClosureKind>`
- `pending_placed_kickoff_outcomes`: `Set<PlacedKickoffObligation>`
- `resolved_placed_kickoff_outcomes`: `Set<PlacedKickoffObligation>`
- `objective_owner_ids`: `Map<String, AgentIdentity>`
- `objective_outcomes`: `Map<String, String>`
- `concluded_objective_ids`: `Set<String>`
- `member_restore_failures`: `Map<AgentIdentity, String>`
- `member_restore_failure_codes`: `Map<AgentIdentity, String>`
- `runtime_retire_refusal_codes`: `Map<AgentRuntimeId, String>`
- `runtime_retire_refusal_reasons`: `Map<AgentRuntimeId, String>`
- `runtime_retire_pending_sessions`: `Map<AgentRuntimeId, SessionId>`
- `remote_runtime_retired_ids`: `Set<AgentRuntimeId>`
- `remote_supervisor_revoked_ids`: `Set<AgentRuntimeId>`
- `member_revival_pending`: `Set<AgentIdentity>`
- `member_run_open`: `Map<AgentIdentity, Bool>`
- `member_in_flight_work`: `Map<AgentIdentity, u64>`
- `member_progress_tokens`: `Map<AgentIdentity, String>`
- `member_last_observed_at_ms`: `Map<AgentIdentity, u64>`
- `member_last_progress_at_ms`: `Map<AgentIdentity, u64>`
- `member_last_progress_event`: `Map<AgentIdentity, MemberProgressEventKind>`
- `member_health_class`: `Map<AgentIdentity, MemberHealthClass>`
- `spawn_exec_phase`: `Map<AgentIdentity, SpawnExecPhase>`
- `member_state_markers`: `Map<AgentRuntimeId, MobMemberState>`
- `wiring_edges`: `Set<WiringEdge>`
- `pending_respawn_topology`: `Set<AgentIdentity>`
- `abandoned_respawn_topology`: `Map<AgentIdentity, Generation>`
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
- `mob_hosts`: `Set<HostId>`
- `host_public_keys`: `Map<HostId, PeerSigningKey>`
- `host_endpoints`: `Map<HostId, PeerAddress>`
- `host_authority_epochs`: `Map<HostId, u64>`
- `host_binding_generations`: `Map<HostId, u64>`
- `host_binding_generation_highwater`: `Map<HostId, u64>`
- `confirmed_host_binding_revocations`: `Set<HostBindingGenerationTombstone>`
- `replacement_host_bind_endpoints`: `Map<HostId, PeerAddress>`
- `replacement_host_binding_generations`: `Map<HostId, u64>`
- `host_bind_phase`: `Map<HostId, HostBindPhase>`
- `host_protocol_min`: `Map<HostId, u64>`
- `host_protocol_max`: `Map<HostId, u64>`
- `host_engine_versions`: `Map<HostId, String>`
- `host_durable_sessions`: `Map<HostId, Bool>`
- `host_autonomous_members`: `Map<HostId, Bool>`
- `host_hard_cancel_member`: `Map<HostId, Bool>`
- `host_tracked_input_cancel`: `Map<HostId, Bool>`
- `host_memory_store`: `Map<HostId, Bool>`
- `host_mcp`: `Map<HostId, Bool>`
- `host_resolvable_providers`: `Map<HostId, Set<String>>`
- `host_approval_forwarding`: `Map<HostId, Bool>`
- `host_live_endpoints`: `Map<HostId, LiveWsEndpointUrl>`
- `member_placement`: `Map<AgentIdentity, HostId>`
- `pending_placed_spawn_ids`: `Map<AgentIdentity, PlacedSpawnId>`
- `pending_placed_spawn_generations`: `Map<AgentIdentity, Generation>`
- `pending_placed_spawn_fence_tokens`: `Map<AgentIdentity, FenceToken>`
- `pending_placed_spawn_hosts`: `Map<AgentIdentity, HostId>`
- `pending_autonomous_placed_spawns`: `Set<AgentIdentity>`
- `pending_placed_spawn_host_binding_generations`: `Map<AgentIdentity, u64>`
- `pending_placed_spawn_spec_digests`: `Map<AgentIdentity, String>`
- `pending_placed_spawn_provision_operation_ids`: `Map<AgentIdentity, String>`
- `pending_placed_spawn_operation_owner_session_ids`: `Map<AgentIdentity, SessionId>`
- `current_placed_spawn_ids`: `Map<AgentIdentity, PlacedSpawnId>`
- `current_placed_spawn_host_binding_generations`: `Map<AgentIdentity, u64>`
- `current_placed_spawn_provision_operation_ids`: `Map<AgentIdentity, String>`
- `current_placed_spawn_operation_owner_session_ids`: `Map<AgentIdentity, SessionId>`
- `pending_placed_carrier_cleanup`: `Set<PlacedCarrierCleanupObligation>`
- `member_materialization_failures`: `Map<AgentIdentity, String>`
- `remote_turn_dispatch_sequence`: `u64`
- `pending_remote_turn_outcomes`: `Set<RemoteTurnObligation>`
- `committed_remote_turn_outcomes`: `Set<RemoteTurnObligation>`
- `resolved_remote_turn_outcomes`: `Set<RemoteTurnObligation>`
- `placed_completion_dispatch_sequence`: `u64`
- `placed_completion_lifecycle_quiescing`: `Bool`
- `placed_completion_lifecycle_intent`: `Option<PlacedCompletionLifecycleIntentKind>`
- `pending_placed_completion_outcomes`: `Set<PlacedCompletionObligation>`
- `cancel_requested_placed_completion_outcomes`: `Set<PlacedCompletionObligation>`
- `resolved_placed_completion_outcomes`: `Set<PlacedCompletionObligation>`
- `pending_route_installs`: `Set<RouteInstallObligation>`
- `operator_grant_scopes`: `Map<PrincipalId, Set<ControlScope>>`
- `operator_grant_expiries`: `Map<PrincipalId, Option<u64>>`
- `spawn_profile_authority_resolved_spec_digests`: `Map<AgentIdentity, String>`

## Inputs
- `RunFlow`(run_id: RunId, step_ids: Set<StepId>, ordered_steps: Seq<StepId>, step_status: Map<StepId, Option<StepRunStatus>>, output_recorded: Map<StepId, Bool>, step_condition_results: Map<StepId, Option<Bool>>, step_has_conditions: Map<StepId, Bool>, step_dependencies: Map<StepId, Seq<StepId>>, step_dependency_modes: Map<StepId, DependencyMode>, step_branches: Map<StepId, Option<BranchId>>, step_collection_policies: Map<StepId, CollectionPolicyKind>, step_quorum_thresholds: Map<StepId, u32>, step_target_counts: Map<StepId, u64>, step_target_success_counts: Map<StepId, u64>, step_target_terminal_failure_counts: Map<StepId, u64>, escalation_threshold: u64, max_step_retries: u32, max_active_nodes: u64, max_active_frames: u64, max_frame_depth: u64)
- `CancelFlow`(run_id: RunId)
- `FlowStatus`
- `CommitSpawnMembership`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, profile_material_digest: String, external_addressable: Bool, runtime_mode: SpawnPolicyRuntimeMode, bridge_session_id: Option<SessionId>, replacing: Option<SessionId>, member_peer_endpoint: Option<MemberPeerEndpoint>, spec_digest_echo: Option<String>, ack_engine_version: Option<String>, placed_spawn_id: Option<PlacedSpawnId>, provision_operation_id: Option<String>)
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
- `SubscribeAllAgentEvents`(session_bound_runtimes: Set<AgentRuntimeId>, external_members: Set<AgentIdentity>)
- `SubscribeMobEvents`(initial_cursor: u64, channel_capacity: u64, poll_interval_ms: u64, session_bound_runtimes: Set<AgentRuntimeId>, external_members: Set<AgentIdentity>)
- `PollEvents`
- `ReplayAllEvents`
- `RecordOperatorActionProvenance`(tool_name: String, principal_token: OpaquePrincipalToken, caller_provenance: Option<MobToolCallerProvenance>, audit_invocation_id: Option<String>)
- `GetMember`
- `SetSpawnPolicy`(enabled: Bool)
- `Shutdown`
- `ForceCancel`(agent_identity: AgentIdentity)
- `ConcludeObjective`(member_id: AgentIdentity, objective_id: String, outcome: String)

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
- `BeginSpawnExec`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, profile_material_digest: String, external_addressable: Bool, runtime_mode: SpawnPolicyRuntimeMode, bridge_session_id: Option<SessionId>, replacing: Option<SessionId>, placement: Option<HostId>, workgraph_required: Bool, rust_bundles_present: Bool, per_spawn_external_tools_present: Bool, mob_default_external_tools_present: Bool, default_llm_client_override_present: Bool, host_surface_mcp_allowlist_present: Bool, inherited_tool_filter_present: Bool, shell_env_present: Bool, mcp_stdio_env_present: Bool, mcp_http_headers_present: Bool, memory_required: Bool, mcp_required: Bool, resume_session_id: Option<SessionId>, placed_spawn_id: Option<PlacedSpawnId>, placed_provision_operation_id: Option<String>, placed_operation_owner_session_id: Option<SessionId>, effective_profile_override_present: Bool, effective_model_override_present: Bool)
- `CommitSpawnActivation`(agent_identity: AgentIdentity)
- `AbortSpawnExec`(agent_identity: AgentIdentity)
- `AuthorizePlacedCarrierCleanup`(obligation: PlacedCarrierCleanupObligation)
- `ResolvePlacedCarrierCleanup`(obligation: PlacedCarrierCleanupObligation)
- `AuthorizeSpawnProfile`(agent_identity: AgentIdentity, profile_name: String, model: String, profile_material_digest: String, tool_config_digest: String, skills_digest: String, provider_params_digest: Option<String>, output_schema_digest: Option<String>, external_addressable: Bool, resolved_spec_digest: Option<String>)
- `ClassifySpawnManyFailure`(observation: MobSpawnManyFailureObservationKind)
- `ClassifyMemberWait`(agent_identity: AgentIdentity)
- `ObserveMemberProgress`(agent_identity: AgentIdentity, run_open: Bool, in_flight_work: u64, progress_token: String, observed_at_ms: u64)
- `ResolveFlowDelegationEdgeAdmission`(from_role: String, to_role: String, rule_verdict: MobFlowDelegationEdgeRuleVerdictKind, mode: MobFlowDelegationEdgeModeKind)
- `ClassifyRemoteMemberRuntimeObservation`(observed_state: MobRemoteMemberRuntimeObservedState)
- `ResolveSpawnMemberAdmission`(manage_scope_present: Bool, profile_scope_contains: Bool, privileged_resume_bridge_session_present: Bool, privileged_resume_session_present: Bool, privileged_backend_present: Bool, privileged_runtime_mode_present: Bool, privileged_launch_mode_present: Bool, privileged_tool_access_policy_present: Bool, privileged_tooling_present: Bool, privileged_auth_binding_present: Bool)
- `ResolveCurrentMobAdmission`(can_manage_mob: Bool)
- `ResolveSpawnToolAdmission`(can_manage_mob: Bool, spawn_profile_scope_present: Bool)
- `ResolveCreateMobAdmission`(can_create_mobs: Bool)
- `ResolveProfileMutationAdmission`(can_mutate_profiles: Bool)
- `ClassifyMemberOperationEligibility`
- `ClassifyBridgeRejectionRecovery`(rejection_cause: MobBridgeRejectionCause)
- `ClassifyPendingSupervisorAcceptance`(rejection_cause: MobBridgeRejectionCause)
- `ClassifyRetirePendingSpawnDisposition`(agent_identity: AgentIdentity)
- `RetireAbsent`(agent_identity: AgentIdentity)
- `RequestPendingSessionIngressDetachForMobDestroy`(mob_id: MobId, agent_runtime_id: AgentRuntimeId)
- `BindOwnerBridgeSession`(bridge_session_id: SessionId, destroy_on_owner_archive: Bool, implicit_delegation_mob: Bool)
- `CleanupRetiringMemberWiring`(edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `RestoreRetiringMemberWiring`(edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `RegisterMemberPeer`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, generation: Generation, fence_token: FenceToken, peer_endpoint: MemberPeerEndpoint)
- `AuthorizeMemberEndpointMigrationTrustCleanup`(edge: WiringEdge, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, retained_peer_endpoint: MemberPeerEndpoint)
- `AuthorizeMemberPeerRebind`(agent_identity: AgentIdentity, expected_peer_endpoint: MemberPeerEndpoint)
- `AuthorizeMemberPeerOverlay`(agent_identity: AgentIdentity, expected_peer_endpoint: MemberPeerEndpoint)
- `AuthorizeRetiringMemberPeerOverlayCleanup`(recipient_identity: AgentIdentity, expected_recipient_endpoint: MemberPeerEndpoint, retiring_identity: AgentIdentity, retiring_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
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
- `BeginHostBind`(host_id: HostId, expected_endpoint: PeerAddress, binding_generation: u64)
- `CommitHostBind`(host_id: HostId, pubkey: PeerSigningKey, endpoint: PeerAddress, epoch: u64, binding_generation: u64, protocol_min: u64, protocol_max: u64, engine_version: String, durable_sessions: Bool, autonomous_members: Bool, hard_cancel_member: Bool, tracked_input_cancel: Bool, memory_store: Bool, mcp: Bool, resolvable_providers: Set<String>, approval_forwarding: Bool, live_endpoint: Option<LiveWsEndpointUrl>)
- `HostRebound`(host_id: HostId, epoch: u64, binding_generation: u64, protocol_min: u64, protocol_max: u64, engine_version: String, durable_sessions: Bool, autonomous_members: Bool, hard_cancel_member: Bool, tracked_input_cancel: Bool, memory_store: Bool, mcp: Bool, resolvable_providers: Set<String>, approval_forwarding: Bool, live_endpoint: Option<LiveWsEndpointUrl>)
- `RefreshHostCapabilities`(host_id: HostId, epoch: u64, binding_generation: u64, protocol_min: u64, protocol_max: u64, engine_version: String, durable_sessions: Bool, autonomous_members: Bool, hard_cancel_member: Bool, tracked_input_cancel: Bool, memory_store: Bool, mcp: Bool, resolvable_providers: Set<String>, approval_forwarding: Bool, live_endpoint: Option<LiveWsEndpointUrl>)
- `RevokeHost`(host_id: HostId, binding_generation: u64)
- `PromoteCommittedPlacedSpawnCarrierBinding`(spawn_id: PlacedSpawnId, agent_identity: AgentIdentity, generation: Generation, fence_token: FenceToken, host_id: HostId, expected_host_binding_generation: u64, host_binding_generation: u64)
- `RecordMemberMaterializationFailure`(agent_identity: AgentIdentity, kind: String)
- `RecordRouteInstall`(obligation: RouteInstallObligation)
- `AuthorizeRouteRemovalBeforeUnwire`(obligation: RouteInstallObligation)
- `ResolveRouteInstall`(obligation: RouteInstallObligation)
- `RollbackRouteInstall`(obligation: RouteInstallObligation)
- `RecordRemoteTurnObligation`(obligation: RemoteTurnObligation)
- `AbortRemoteTurnObligation`(obligation: RemoteTurnObligation)
- `CommitRemoteTurnOutcome`(obligation: RemoteTurnObligation)
- `ResolveRemoteTurnObligation`(obligation: RemoteTurnObligation)
- `AcknowledgeRemoteTurnOutcome`(obligation: RemoteTurnObligation)
- `DisposeRemoteTurnObligation`(obligation: RemoteTurnObligation)
- `RecordPlacedCompletionObligation`(obligation: PlacedCompletionObligation)
- `RequestPlacedCompletionCancellation`(obligation: PlacedCompletionObligation)
- `ResolvePlacedCompletionOutcome`(obligation: PlacedCompletionObligation)
- `ClosePlacedCompletionOutcome`(obligation: PlacedCompletionObligation)
- `AcknowledgePlacedCompletionOutcome`(obligation: PlacedCompletionObligation)
- `DisposePlacedCompletionOutcome`(obligation: PlacedCompletionObligation)
- `BeginPlacedCompletionLifecycleQuiesce`(intent: PlacedCompletionLifecycleIntentKind)
- `EndPlacedCompletionLifecycleQuiesce`(intent: PlacedCompletionLifecycleIntentKind)
- `GrantOperatorScopes`(principal: PrincipalId, scopes: Set<ControlScope>, expires_at_ms: Option<u64>)
- `RevokeOperatorScopes`(principal: PrincipalId, revoked: Set<ControlScope>, remaining: Set<ControlScope>)
- `ResolveMemberOperatorAdmission`(agent_identity: AgentIdentity, requester_generation: Generation, requester_fence_token: FenceToken, requester_host_id: HostId, requester_host_binding_generation: u64, requester_member_session_id: SessionId, sender_peer_id: PeerId, request_id: String)
- `ClassifyFlowStepDispatch`(run_id: RunId, step_id: StepId, target: AgentIdentity, overlay_present: Bool)
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
- `KickoffMarkPending`(member_id: AgentIdentity, objective_id: String)
- `BindObjectiveOwner`(owner_id: AgentIdentity, objective_id: String)
- `KickoffMarkStarting`(member_id: AgentIdentity)
- `StartPlacedKickoff`(obligation: PlacedKickoffObligation)
- `ResolvePlacedKickoffStarted`(obligation: PlacedKickoffObligation)
- `ResolvePlacedKickoffCallbackPending`(obligation: PlacedKickoffObligation)
- `ResolvePlacedKickoffFailed`(obligation: PlacedKickoffObligation, error: String)
- `ResolvePlacedKickoffCancelled`(obligation: PlacedKickoffObligation)
- `RejectPlacedKickoffBeforeAdmission`(obligation: PlacedKickoffObligation, error: String)
- `AcknowledgePlacedKickoffOutcome`(obligation: PlacedKickoffObligation)
- `DisposePlacedKickoffObligation`(obligation: PlacedKickoffObligation)
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
- `RetireMember`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, session_id: Option<SessionId>)
- `AdmitDestroyMemberRetire`(mob_id: MobId, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, session_id: Option<SessionId>)
- `ObserveRuntimeRetired`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `ObserveMemberRetirementArchived`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, session_id: Option<SessionId>, disposal: MemberSessionDisposal, preserve_machine_topology: Bool)
- `ObserveRemoteMemberRetirementArchivedAndSupervisorRevoked`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, preserve_machine_topology: Bool)
- `ObserveDestroyMemberRetirementArchived`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, session_id: Option<SessionId>, disposal: MemberSessionDisposal)
- `ResolveRespawnTopologyRestore`(agent_identity: AgentIdentity, failed_peer_ids: Seq<RespawnTopologyPeerId>)
- `DestroyMob`(session_id: SessionId)
- `ObserveRuntimeDestroyed`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `RecoverRosterMember`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, profile_name: String, runtime_mode: SpawnPolicyRuntimeMode, external_addressable: Bool)
- `RecoverMemberSessionBinding`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, bridge_session_id: SessionId, replacing: Option<SessionId>)
- `RecoverSpawnedMemberPeerEndpoint`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, peer_endpoint: MemberPeerEndpoint)
- `RecoverMemberPeerEndpoint`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, bridge_session_id: SessionId, peer_endpoint: MemberPeerEndpoint)
- `RecoverRosterMemberReset`(agent_identity: AgentIdentity, previous_agent_runtime_id: AgentRuntimeId, previous_generation: Generation, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `RecoverRosterMemberRetiring`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, session_id: Option<SessionId>, host_id: Option<HostId>)
- `RecoverRosterMemberRetirementStarted`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, generation: Generation, releasing: Option<SessionId>, session_id: Option<SessionId>, retiring_peer_endpoint: Option<MemberPeerEndpoint>, preserve_machine_topology: Bool)
- `ObserveRespawnTopologyPreservationStarted`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `ObserveRespawnTopologyAbandoned`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `RecoverRemoteMemberRuntimeRetired`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `RecoverRemoteMemberSupervisorRevoked`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `RecoverRosterMemberRetired`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, generation: Generation, preserve_machine_topology: Bool, preservation_started: Bool)
- `ConvergeRecoveredRosterTopology`(edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity)
- `ConvergeRecoveredRespawnTopology`(agent_identity: AgentIdentity)
- `RecoverMemberKickoff`(member_id: AgentIdentity, phase: KickoffPhase, error: Option<String>)
- `RecoverObjectiveBinding`(member_id: AgentIdentity, objective_id: String)
- `RecoverFinalizedPlacedKickoff`(obligation: PlacedKickoffObligation, outcome_kind: PlacedKickoffOutcomeKind, outcome_error: Option<String>, closure_kind: PlacedKickoffClosureKind)
- `RecoverObjectiveConclusion`(member_id: AgentIdentity, objective_id: String, outcome: String)
- `RecoverRosterWiring`(edge: WiringEdge)
- `RecoverRosterUnwire`(edge: WiringEdge)
- `RecoverExternalPeerWiring`(key: ExternalPeerKey, edge: ExternalPeerEdge)
- `RecoverExternalPeerUnwire`(key: ExternalPeerKey)
- `RecoverSupervisorAuthority`(peer_id: PeerId, signing_key: PeerSigningKey, epoch: u64, protocol_version: SupervisorProtocolVersion, pending_operation_id: Option<String>, pending_peer_id: Option<PeerId>, pending_signing_key: Option<PeerSigningKey>, pending_epoch: Option<u64>, pending_protocol_version: Option<SupervisorProtocolVersion>, pending_accepted_peer_ids: Set<PeerId>, pending_member_target_names: Map<PeerId, String>, pending_member_target_addresses: Map<PeerId, String>)
- `RecoverHostBindRequest`(host_id: HostId, expected_endpoint: PeerAddress, binding_generation: u64, replacement: Bool)
- `RecoverHostBinding`(host_id: HostId, pubkey: PeerSigningKey, endpoint: PeerAddress, epoch: u64, binding_generation: u64, protocol_min: u64, protocol_max: u64, engine_version: String, durable_sessions: Bool, autonomous_members: Bool, hard_cancel_member: Bool, tracked_input_cancel: Bool, memory_store: Bool, mcp: Bool, resolvable_providers: Set<String>, approval_forwarding: Bool, live_endpoint: Option<LiveWsEndpointUrl>)
- `RecoverHostBindingGenerationHighwater`(host_id: HostId, binding_generation: u64)
- `RecoverConfirmedHostBindingRevocation`(host_id: HostId, binding_generation: u64)
- `RecoverRemoteTurnDispatchSequence`(dispatch_sequence: u64)
- `RecoverPlacedCompletionDispatchSequence`(dispatch_sequence: u64)
- `RecoverPendingPlacedCompletion`(obligation: PlacedCompletionObligation, cancellation_requested: Bool)
- `RecoverResolvedPlacedCompletion`(obligation: PlacedCompletionObligation)
- `RecoverCompletedWithCleanupCustody`
- `RecoverOwnerBridgeSession`(bridge_session_id: SessionId, destroy_on_owner_archive: Bool, implicit_delegation_mob: Bool)
- `RecoverMemberRestoreFailure`(agent_identity: AgentIdentity, reason: String)
- `RecoverPendingPlacedSpawn`(spawn_id: PlacedSpawnId, agent_identity: AgentIdentity, generation: Generation, fence_token: FenceToken, host_id: HostId, host_binding_generation: u64, spec_digest: String, runtime_mode: SpawnPolicyRuntimeMode, provision_operation_id: String, operation_owner_session_id: SessionId)
- `RecoverCommittedPlacedSpawn`(spawn_id: PlacedSpawnId, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, generation: Generation, fence_token: FenceToken, host_id: HostId, host_binding_generation: u64, member_session_id: SessionId, member_peer_endpoint: MemberPeerEndpoint, profile_name: String, runtime_mode: SpawnPolicyRuntimeMode, external_addressable: Bool, provision_operation_id: String, operation_owner_session_id: SessionId)
- `RecoverPlacedCarrierCleanup`(obligation: PlacedCarrierCleanupObligation)
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
- `SpawnProfileAuthorized`(agent_identity: AgentIdentity, profile_name: String, model: String, profile_material_digest: String, tool_config_digest: String, skills_digest: String, provider_params_digest: Option<String>, output_schema_digest: Option<String>, external_addressable: Bool, resolved_spec_digest: Option<String>)
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
- `PersistPlacedCompletionLifecycleIntent`(intent: PlacedCompletionLifecycleIntentKind, active: Bool)
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
- `PersistObjectiveOwnerBinding`(owner_id: AgentIdentity, objective_id: String)
- `PersistObjectiveConclusion`(member_id: AgentIdentity, objective_id: String, outcome: String)
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
- `RetirePendingSpawnCancellationAuthorized`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, generation: Generation, pending_spawn_session_id: SessionId)
- `RetireCommittedIncarnationWithoutPendingSpawnResolved`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, generation: Generation)
- `RetireAbsentPendingSpawnPreservationResolved`(agent_identity: AgentIdentity)
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
- `AuthorizeAllAgentEventSubscription`(session_bound_runtimes: Set<AgentRuntimeId>, external_members: Set<AgentIdentity>)
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
- `RequestHostBind`(host_id: HostId, endpoint: PeerAddress, binding_generation: u64)
- `HostRegistered`(host_id: HostId, epoch: u64, binding_generation: u64)
- `HostReboundRecorded`(host_id: HostId, epoch: u64, binding_generation: u64)
- `HostCapabilitiesRefreshed`(host_id: HostId, epoch: u64, binding_generation: u64)
- `HostRevoked`(host_id: HostId, binding_generation: u64)
- `PersistPendingPlacedSpawn`(spawn_id: PlacedSpawnId, agent_identity: AgentIdentity, generation: Generation, fence_token: FenceToken, host: HostId, host_binding_generation: u64, spec_digest: String, effective_profile_override_present: Bool, effective_model_override_present: Bool, provision_operation_id: String, operation_owner_session_id: SessionId)
- `RequestMemberMaterialization`(spawn_id: PlacedSpawnId, agent_identity: AgentIdentity, generation: Generation, fence_token: FenceToken, spec_digest: String, host: HostId, host_binding_generation: u64, provision_operation_id: String, operation_owner_session_id: SessionId)
- `CommitPlacedSpawnCarrier`(spawn_id: PlacedSpawnId, agent_identity: AgentIdentity, generation: Generation, fence_token: FenceToken, host: HostId, host_binding_generation: u64, spec_digest: String, member_session_id: SessionId, member_peer_endpoint: MemberPeerEndpoint, ack_engine_version: String, provision_operation_id: String, operation_owner_session_id: SessionId)
- `PromoteCommittedPlacedSpawnCarrierBinding`(spawn_id: PlacedSpawnId, agent_identity: AgentIdentity, generation: Generation, fence_token: FenceToken, host: HostId, expected_host_binding_generation: u64, host_binding_generation: u64)
- `PlacedCarrierCleanupRequested`(obligation: PlacedCarrierCleanupObligation)
- `PlacedCarrierCleanupAuthorized`(obligation: PlacedCarrierCleanupObligation)
- `PlacedCarrierCleanupResolved`(obligation: PlacedCarrierCleanupObligation)
- `RequestMemberRelease`(agent_identity: AgentIdentity, generation: Generation, fence_token: FenceToken, host: HostId)
- `RouteInstallRequested`(obligation: RouteInstallObligation)
- `MemberOperatorAdmitted`(agent_identity: AgentIdentity, request_id: String)
- `MemberOperatorRejected`(agent_identity: AgentIdentity, request_id: String, cause: MemberOperatorRejectKind)
- `FlowStepDispatchClassified`(run_id: RunId, step_id: StepId, target: AgentIdentity, dispatch: FlowStepDispatchKind)
- `AuthorizeExternalAgentEventSubscription`(agent_identity: AgentIdentity, host: HostId)
- `GrantRecorded`(principal: PrincipalId, scopes: Set<ControlScope>, expires_at_ms: Option<u64>)
- `GrantRevoked`(principal: PrincipalId, revoked: Set<ControlScope>, remaining: Set<ControlScope>)

## Command Plans
### `AuthorizedMobSpawnStart`
- Authority: `PendingSpawnOperationOwnerAuthorized`
- Source Inputs: `CancelPendingSpawn`
- Source Signals: `StageSpawn`, `CompleteSpawn`
- Transitions: `StageSpawnRunning`, `CompleteSpawnRunning`, `CompleteSpawnLateArrivalRunning`, `CompleteSpawnLateArrivalStopped`, `CompleteSpawnLateArrivalCompleted`, `CompleteSpawnDestroyed`, `CancelPendingSpawnPresentRunning`, `CancelPendingSpawnPresentStopped`, `CancelPendingSpawnPresentCompleted`, `CancelPendingSpawnAbsentRunning`, `CancelPendingSpawnAbsentStopped`, `CancelPendingSpawnAbsentCompleted`, `CancelPendingSpawnDestroyed`
- Guard Expansion:
  - `StageSpawnRunning`: `lifecycle_origin_open`, `pending_identity_unused`
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
  - `StageSpawnRunning`: `lifecycle_origin_open`, `pending_identity_unused`
- Command Effects: `PendingSpawnOperationOwnerAuthorized`
- Effect Closure:
  - `PendingSpawnOperationOwnerAuthorized` via `CanStartSpawn` (LocalPendingSpawnOwner) states: `Authorized`, `Attempted`, `Realized`, `Failed`, `Cancelled`, `Abandoned`
- Emitted By Transitions: `ExposePendingSpawn`, `PendingSpawnOperationOwnerAuthorized`

### `SpawnStarted`
- Authority: `SpawnStarted`
- Source Signals: `StageSpawn`
- Transitions: `StageSpawnRunning`
- Guard Expansion:
  - `StageSpawnRunning`: `lifecycle_origin_open`, `pending_identity_unused`
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
  - `StageSpawnRunning`: `lifecycle_origin_open`, `pending_identity_unused`
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
- `placed_spawn_pending_attempt_is_complete`
- `pending_autonomous_placed_spawn_is_an_exact_pending_attempt`
- `placed_spawn_committed_attempt_is_complete`
- `placed_spawn_attempt_is_not_both_pending_and_committed`
- `identity_runtime_history_is_coherent`
- `current_runtime_binding_has_history`
- `pending_respawn_topology_has_no_current_runtime`
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
- `placement_never_targets_half_bound_host`
- `placed_members_require_owner_bridge`
- `live_endpoints_subset_of_bound_hosts`
- `host_epochs_present_for_hosts`
- `host_binding_generations_cover_tracked_hosts`
- `replacement_host_bind_requests_are_exact_and_disjoint`
- `host_capability_maps_cover_bound_hosts`
- `remote_committed_members_have_peer_endpoints`
- `live_member_peer_ids_are_unique`
- `live_member_peer_ids_are_disjoint_from_hosts`
- `remote_turn_custody_sets_are_pairwise_disjoint`
- `placed_completion_custody_is_well_partitioned`
- `placed_completion_lifecycle_intent_matches_quiesce`
- `placed_completion_lifecycle_intent_owns_no_adaptive_custody`
- `placed_completion_custody_is_input_and_sequence_injective`
- `placed_kickoff_custody_sets_are_disjoint`
- `placed_kickoff_custody_is_identity_and_input_injective`
- `retained_placed_kickoff_correlations_are_injective_and_live`
- `placed_kickoff_and_flow_correlations_are_disjoint`
- `placed_completion_correlations_are_disjoint`
- `pending_route_ledger_is_install_only`
- `remote_turn_custody_requires_exact_current_placement`
- `placed_completion_custody_requires_exact_current_placement`
- `placed_kickoff_custody_requires_exact_current_placement`
- `remote_turn_custody_sequences_are_bounded_and_injective`
- `placed_completion_sequences_are_bounded`

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

### `ObserveMemberProgressChangedOpenRunning`
- From: `Running`
- On: `ObserveMemberProgress`(agent_identity, run_open, in_flight_work, progress_token, observed_at_ms)
- Guards:
  - `observation_monotonic`
  - `member_progress_changed`
  - `run_open`
- To: `Running`

### `ObserveMemberProgressChangedIdleRunning`
- From: `Running`
- On: `ObserveMemberProgress`(agent_identity, run_open, in_flight_work, progress_token, observed_at_ms)
- Guards:
  - `observation_monotonic`
  - `member_progress_changed`
  - `run_idle`
- To: `Running`

### `ObserveMemberProgressUnchangedIdleRunning`
- From: `Running`
- On: `ObserveMemberProgress`(agent_identity, run_open, in_flight_work, progress_token, observed_at_ms)
- Guards:
  - `observation_monotonic`
  - `member_progress_unchanged`
  - `run_idle`
- To: `Running`

### `ObserveMemberProgressUnchangedOpenHealthyRunning`
- From: `Running`
- On: `ObserveMemberProgress`(agent_identity, run_open, in_flight_work, progress_token, observed_at_ms)
- Guards:
  - `observation_monotonic`
  - `member_progress_unchanged`
  - `run_open`
  - `progress_recent`
- To: `Running`

### `ObserveMemberProgressUnchangedOpenDegradedRunning`
- From: `Running`
- On: `ObserveMemberProgress`(agent_identity, run_open, in_flight_work, progress_token, observed_at_ms)
- Guards:
  - `observation_monotonic`
  - `member_progress_unchanged`
  - `run_open`
  - `progress_degraded`
- To: `Running`

### `ObserveMemberProgressUnchangedOpenWedgedRunning`
- From: `Running`
- On: `ObserveMemberProgress`(agent_identity, run_open, in_flight_work, progress_token, observed_at_ms)
- Guards:
  - `observation_monotonic`
  - `member_progress_unchanged`
  - `run_open`
  - `progress_wedged`
- To: `Running`

### `ObserveMemberProgressStaleRunning`
- From: `Running`
- On: `ObserveMemberProgress`(agent_identity, run_open, in_flight_work, progress_token, observed_at_ms)
- Guards:
  - `observation_stale`
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
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `manage_scope_allows`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `ResolveSpawnMemberAdmissionManageScopeStopped`
- From: `Stopped`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `manage_scope_allows`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Stopped`

### `ResolveSpawnMemberAdmissionManageScopeCompleted`
- From: `Completed`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `manage_scope_allows`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Completed`

### `ResolveSpawnMemberAdmissionManageScopeDestroyed`
- From: `Destroyed`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `manage_scope_allows`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Destroyed`

### `ResolveSpawnMemberAdmissionPrivilegedArgsDeniedRunning`
- From: `Running`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `privileged_args_without_manage_scope`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `ResolveSpawnMemberAdmissionPrivilegedArgsDeniedStopped`
- From: `Stopped`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `privileged_args_without_manage_scope`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Stopped`

### `ResolveSpawnMemberAdmissionPrivilegedArgsDeniedCompleted`
- From: `Completed`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `privileged_args_without_manage_scope`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Completed`

### `ResolveSpawnMemberAdmissionPrivilegedArgsDeniedDestroyed`
- From: `Destroyed`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `privileged_args_without_manage_scope`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Destroyed`

### `ResolveSpawnMemberAdmissionProfileScopeRunning`
- From: `Running`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `profile_scope_allows`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `ResolveSpawnMemberAdmissionProfileScopeStopped`
- From: `Stopped`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `profile_scope_allows`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Stopped`

### `ResolveSpawnMemberAdmissionProfileScopeCompleted`
- From: `Completed`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `profile_scope_allows`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Completed`

### `ResolveSpawnMemberAdmissionProfileScopeDestroyed`
- From: `Destroyed`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `profile_scope_allows`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Destroyed`

### `ResolveSpawnMemberAdmissionDeniedRunning`
- From: `Running`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `no_scope_denies`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `ResolveSpawnMemberAdmissionDeniedStopped`
- From: `Stopped`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `no_scope_denies`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Stopped`

### `ResolveSpawnMemberAdmissionDeniedCompleted`
- From: `Completed`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_tooling_present, privileged_auth_binding_present)
- Guards:
  - `no_scope_denies`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Completed`

### `ResolveSpawnMemberAdmissionDeniedDestroyed`
- From: `Destroyed`
- On: `ResolveSpawnMemberAdmission`(manage_scope_present, profile_scope_contains, privileged_resume_bridge_session_present, privileged_resume_session_present, privileged_backend_present, privileged_runtime_mode_present, privileged_launch_mode_present, privileged_tool_access_policy_present, privileged_tooling_present, privileged_auth_binding_present)
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
  - `lifecycle_origin_open`
  - `no_active_adaptive_run`
  - `adaptive_run_id_unused`
  - `prior_adaptive_custody_drained`
  - `limits_complete`
- Emits: `AdaptiveRunInitialized`
- To: `Running`

### `InitializeAdaptiveRunReplayRunning`
- From: `Running`
- On: `InitializeAdaptiveRun`(adaptive_run_id, max_depth, max_total_decisions, max_repair_attempts, max_layer_failures, max_attempts_per_layer, max_members_per_layer, max_total_spawned_members, max_active_members, max_retained_layer_mobs, max_aggregate_tokens, max_aggregate_tool_calls, allowed_model_classes, allowed_tool_classes, allowed_skill_identities, allowed_auth_binding_refs, deadline_ms)
- Guards:
  - `lifecycle_origin_open`
  - `same_active_run`
  - `run_still_unstarted`
  - `exact_initialization_replay`
- Emits: `AdaptiveRunInitialized`
- To: `Running`

### `RecordAdaptivePlanningDecisionActiveRunning`
- From: `Running`
- On: `RecordPlanningDecision`(adaptive_run_id, decision_kind)
- Guards:
  - `lifecycle_origin_open`
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
  - `adaptive_run_custody_drained`
- Emits: `AdaptiveRunTerminalized`
- To: `Running`

### `RecordAdaptivePlanRejectedActiveRunning`
- From: `Running`
- On: `RecordPlanRejected`(adaptive_run_id, layer_id)
- Guards:
  - `lifecycle_origin_open`
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
  - `adaptive_run_custody_drained`
- Emits: `AdaptiveRunTerminalized`
- To: `Running`

### `ResolveAdaptiveLayerAdmissionAllowedRunning`
- From: `Running`
- On: `ResolveLayerAdmission`(adaptive_run_id, layer_id, attempt, plan_digest, child_mob_id, member_count, token_reservation, tool_call_reservation, used_model_classes, used_tool_classes, used_skill_identities, used_auth_binding_refs, observed_at_ms)
- Guards:
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
  - `run_active`
  - `limit_exceeded`
- Emits: `AdaptiveLayerAdmissionResolved`
- To: `Running`

### `RecordAdaptiveLayerProvisionedRunning`
- From: `Running`
- On: `RecordLayerProvisioned`(adaptive_run_id, layer_id, attempt)
- Guards:
  - `layer_owner_matches`
  - `active_layer_matches`
  - `layer_admitted`
  - `attempt_matches`
- Emits: `AdaptiveLayerProvisioned`
- To: `Running`

### `RecordAdaptiveLayerRunStartedRunning`
- From: `Running`
- On: `RecordLayerRunStarted`(adaptive_run_id, layer_id, attempt, child_run_id)
- Guards:
  - `lifecycle_origin_open`
  - `layer_owner_matches`
  - `active_layer_matches`
  - `layer_provisioning`
  - `attempt_matches`
- Emits: `AdaptiveLayerRunStarted`
- To: `Running`

### `IngestAdaptiveLayerTerminalSuccessRunning`
- From: `Running`
- On: `IngestLayerTerminal`(adaptive_run_id, layer_id, attempt, result_class, actual_tokens, actual_tool_calls)
- Guards:
  - `layer_owner_matches`
  - `active_layer_matches`
  - `layer_running`
  - `attempt_matches`
  - `result_success`
- Emits: `AdaptiveLayerTerminalIngested`
- To: `Running`

### `IngestAdaptiveLayerTerminalFailureRunning`
- From: `Running`
- On: `IngestLayerTerminal`(adaptive_run_id, layer_id, attempt, result_class, actual_tokens, actual_tool_calls)
- Guards:
  - `layer_owner_matches`
  - `active_layer_matches`
  - `layer_running`
  - `attempt_matches`
  - `result_failure`
- Emits: `AdaptiveLayerTerminalIngested`
- To: `Running`

### `RecordAdaptiveLayerSetupFaultRunning`
- From: `Running`
- On: `RecordLayerSetupFault`(adaptive_run_id, layer_id, attempt, fault, spawned_members, requested_members)
- Guards:
  - `layer_owner_matches`
  - `active_layer_matches`
  - `layer_admitted_or_provisioning`
  - `attempt_matches`
  - `setup_counts_valid`
- Emits: `AdaptiveLayerSetupFaultRecorded`
- To: `Running`

### `RecordAdaptiveLayerResultValidatedRunning`
- From: `Running`
- On: `RecordLayerResultValidated`(adaptive_run_id, layer_id, attempt, result_digest)
- Guards:
  - `layer_owner_matches`
  - `active_layer_matches`
  - `layer_collecting`
  - `attempt_matches`
- Emits: `AdaptiveLayerResultValidated`
- To: `Running`

### `RecordAdaptiveLayerResultInvalidRunning`
- From: `Running`
- On: `RecordLayerResultInvalid`(adaptive_run_id, layer_id, attempt)
- Guards:
  - `layer_owner_matches`
  - `active_layer_matches`
  - `layer_collecting`
  - `attempt_matches`
- Emits: `AdaptiveLayerResultInvalid`
- To: `Running`

### `RecordAdaptiveLayerMobDestroyedRunning`
- From: `Running`
- On: `RecordLayerMobDestroyed`(adaptive_run_id, layer_id, attempt)
- Guards:
  - `layer_owner_matches`
  - `layer_terminal`
  - `attempt_matches`
  - `disposition_absent`
  - `active_member_debit_present`
- Emits: `AdaptiveLayerCleanupObserved`
- To: `Running`

### `RecordAdaptiveLayerMobDestroyedReplayRunning`
- From: `Running`
- On: `RecordLayerMobDestroyed`(adaptive_run_id, layer_id, attempt)
- Guards:
  - `layer_owner_matches`
  - `attempt_matches`
  - `exact_disposition_replay`
- To: `Running`

### `RecordAdaptiveLayerMobRetainedRunning`
- From: `Running`
- On: `RecordLayerMobRetained`(adaptive_run_id, layer_id, attempt, disposition)
- Guards:
  - `retained_disposition`
  - `layer_owner_matches`
  - `layer_terminal`
  - `attempt_matches`
  - `disposition_absent`
  - `retention_capacity_available`
  - `active_member_debit_present`
- Emits: `AdaptiveLayerCleanupObserved`
- To: `Running`

### `RecordAdaptiveLayerMobRetainedCleanupRequiredRunning`
- From: `Running`
- On: `RecordLayerMobRetained`(adaptive_run_id, layer_id, attempt, disposition)
- Guards:
  - `retained_disposition`
  - `layer_owner_matches`
  - `layer_terminal`
  - `attempt_matches`
  - `disposition_absent`
  - `retention_capacity_exhausted`
  - `active_member_debit_present`
- Emits: `AdaptiveLayerCleanupObserved`
- To: `Running`

### `RecordAdaptiveLayerMobRetainedReplayRunning`
- From: `Running`
- On: `RecordLayerMobRetained`(adaptive_run_id, layer_id, attempt, disposition)
- Guards:
  - `retained_disposition`
  - `layer_owner_matches`
  - `attempt_matches`
  - `exact_disposition_replay`
- To: `Running`

### `RecordAdaptiveCleanupResolvedRunning`
- From: `Running`
- On: `RecordCleanupResolved`(adaptive_run_id)
- Guards:
  - `lifecycle_origin_open`
  - `cleanup_pause_active`
- Emits: `AdaptiveCleanupResolved`
- To: `Running`

### `RecordAdaptiveBodyEvidenceMissingRunning`
- From: `Running`
- On: `RecordBodyEvidenceMissing`(adaptive_run_id, missing_digest)
- Guards:
  - `run_active`
  - `adaptive_run_custody_drained`
- Emits: `AdaptiveBodyEvidenceMissing`
- To: `Running`

### `ResolveAdaptiveFinishRunningRunning`
- From: `Running`
- On: `ResolveAdaptiveFinish`(adaptive_run_id, final_result_digest)
- Guards:
  - `run_active`
  - `adaptive_run_custody_drained`
- Emits: `AdaptiveRunTerminalized`
- To: `Running`

### `RequestAdaptiveCancelRunningRunning`
- From: `Running`
- On: `RequestAdaptiveCancel`(adaptive_run_id)
- Guards:
  - `run_cancelable`
- Emits: `AdaptiveRunTerminalized`
- To: `Running`

### `RequestAdaptiveCancelReplayRunning`
- From: `Running`
- On: `RequestAdaptiveCancel`(adaptive_run_id)
- Guards:
  - `already_terminal`
- To: `Running`

### `RecordAdaptiveDeadlineObservedExpiredRunning`
- From: `Running`
- On: `RecordDeadlineObserved`(adaptive_run_id, observed_at_ms)
- Guards:
  - `run_active`
  - `deadline_expired`
  - `adaptive_run_custody_drained`
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
  - `lifecycle_origin_open`
- Emits: `MemberOperationEligibilityResolved`
- To: `Running`

### `ClassifyMemberOperationEligibilityRunningDestroyDeniedRunning`
- From: `Running`
- On: `ClassifyMemberOperationEligibility`()
- Guards:
  - `running_but_destroying_is_denied`
- Emits: `MemberOperationEligibilityResolved`
- To: `Running`

### `ClassifyMemberOperationEligibilityRunningLifecycleDeniedRunning`
- From: `Running`
- On: `ClassifyMemberOperationEligibility`()
- Guards:
  - `not_destroying`
  - `lifecycle_origin_closed`
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
- On: `CommitSpawnMembership`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, member_peer_endpoint, spec_digest_echo, ack_engine_version, placed_spawn_id, provision_operation_id)
- Guards:
  - `spawn_exec_opened`
  - `ack_fields_absent`
  - `no_prior_session_binding`
  - `current_binding_absent`
  - `generation_is_next`
  - `replacing_absent`
  - `bridge_session_present`
- Emits: `RequestRuntimeBinding`, `AppendLifecycleJournal`, `MemberSessionBindingChanged`, `EmitMemberLifecycleNotice`
- To: `Running`

### `CommitSpawnMembershipFreshPeerOnly`
- From: `Running`
- On: `CommitSpawnMembership`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, member_peer_endpoint, spec_digest_echo, ack_engine_version, placed_spawn_id, provision_operation_id)
- Guards:
  - `spawn_exec_opened`
  - `ack_fields_absent`
  - `no_prior_session_binding`
  - `current_binding_absent`
  - `generation_is_next`
  - `replacing_absent`
  - `bridge_session_absent`
- Emits: `AppendLifecycleJournal`, `EmitMemberLifecycleNotice`
- To: `Running`

### `CommitSpawnMembershipRemote`
- From: `Running`
- On: `CommitSpawnMembership`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, member_peer_endpoint, spec_digest_echo, ack_engine_version, placed_spawn_id, provision_operation_id)
- Guards:
  - `spawn_exec_materialize_pending`
  - `placement_present`
  - `no_prior_session_binding`
  - `current_binding_absent`
  - `generation_is_next`
  - `replacing_absent`
  - `bridge_session_present`
  - `ack_endpoint_present`
  - `ack_peer_id_unowned`
  - `ack_peer_id_is_not_host`
  - `placed_spawn_id_matches`
  - `placed_generation_matches`
  - `placed_fence_matches`
  - `placed_host_matches`
  - `placed_host_binding_generation_current`
  - `provision_operation_id_matches_pending`
  - `spec_digest_echo_matches`
  - `engine_version_matches_bound_host`
- Emits: `CommitPlacedSpawnCarrier`, `AppendLifecycleJournal`, `MemberSessionBindingChanged`, `MemberPeerRegistered`, `EmitMemberLifecycleNotice`
- To: `Running`

### `BeginSpawnExecFresh`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `lifecycle_origin_open`
  - `placement_absent`
  - `placed_spawn_attempt_absent`
  - `effective_profile_override_presence_local`
  - `effective_model_override_presence_local`
  - `no_pending_placed_spawn`
  - `no_current_placed_spawn`
  - `no_placed_carrier_cleanup`
  - `coordinator_bound`
  - `spawn_exec_settled`
  - `no_prior_session_binding`
  - `current_binding_absent`
  - `generation_is_next`
  - `replacing_absent`
  - `identity_not_external_peer`
  - `spawn_profile_name_authorized`
  - `spawn_profile_authorized`
  - `spawn_profile_addressability_authorized`
  - `bridge_session_present`
- To: `Running`

### `BeginSpawnExecFreshPeerOnly`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `lifecycle_origin_open`
  - `placement_absent`
  - `placed_spawn_attempt_absent`
  - `effective_profile_override_presence_local`
  - `effective_model_override_presence_local`
  - `no_pending_placed_spawn`
  - `no_current_placed_spawn`
  - `no_placed_carrier_cleanup`
  - `coordinator_bound`
  - `spawn_exec_settled`
  - `no_prior_session_binding`
  - `current_binding_absent`
  - `generation_is_next`
  - `replacing_absent`
  - `identity_not_external_peer`
  - `spawn_profile_name_authorized`
  - `spawn_profile_authorized`
  - `spawn_profile_addressability_authorized`
  - `bridge_session_absent`
- To: `Running`

### `BeginSpawnExecDeniedNonPortableRustBundles`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `placement_present`
  - `rust_bundles_present`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `BeginSpawnExecDeniedNonPortablePerSpawnExternalTools`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `placement_present`
  - `per_spawn_external_tools_present`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `BeginSpawnExecDeniedNonPortableMobDefaultExternalTools`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `placement_present`
  - `mob_default_external_tools_present`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `BeginSpawnExecDeniedNonPortableDefaultLlmClientOverride`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `placement_present`
  - `default_llm_client_override_present`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `BeginSpawnExecDeniedNonPortableHostSurfaceMcpAllowlist`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `placement_present`
  - `host_surface_mcp_allowlist_present`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `BeginSpawnExecDeniedNonPortableInheritedToolFilter`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `placement_present`
  - `inherited_tool_filter_present`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `BeginSpawnExecDeniedNonPortableWorkgraphTools`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `placement_present`
  - `workgraph_required`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `BeginSpawnExecDeniedSecretBearingShellEnv`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `placement_present`
  - `shell_env_present`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `BeginSpawnExecDeniedSecretBearingMcpStdioEnv`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `placement_present`
  - `mcp_stdio_env_present`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `BeginSpawnExecDeniedSecretBearingMcpHttpHeaders`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `placement_present`
  - `mcp_http_headers_present`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `BeginSpawnExecDeniedHostNotBound`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `placement_present`
  - `portability_clean`
  - `host_not_bound`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `BeginSpawnExecDeniedMissingCapabilityAutonomousMembers`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `placement_present`
  - `portability_clean`
  - `host_bound`
  - `host_binding_generation_positive`
  - `autonomous_members_missing`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `BeginSpawnExecDeniedMissingCapabilityDurableSessions`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `placement_present`
  - `portability_clean`
  - `host_bound`
  - `autonomous_members_satisfied`
  - `durable_sessions_missing`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `BeginSpawnExecDeniedMissingCapabilityTrackedInputCancel`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `placement_present`
  - `portability_clean`
  - `host_bound`
  - `autonomous_members_satisfied`
  - `tracked_input_cancel_missing`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `BeginSpawnExecDeniedMissingCapabilityProtocolV4`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `placement_present`
  - `portability_clean`
  - `host_bound`
  - `tracked_turn_capabilities_satisfied`
  - `protocol_v4_missing`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `BeginSpawnExecDeniedMissingCapabilityMemoryStore`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `placement_present`
  - `portability_clean`
  - `host_bound`
  - `autonomous_members_satisfied`
  - `memory_store_missing`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `BeginSpawnExecDeniedMissingCapabilityMcp`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `placement_present`
  - `portability_clean`
  - `host_bound`
  - `autonomous_members_satisfied`
  - `memory_store_satisfied`
  - `mcp_missing`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `BeginSpawnExecDeniedOwnerBridgeAbsent`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `placement_present`
  - `portability_clean`
  - `host_bound`
  - `autonomous_members_satisfied`
  - `memory_store_satisfied`
  - `mcp_satisfied`
  - `owner_bridge_absent`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `BeginSpawnExecDeniedResumePlacementMismatch`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `placement_present`
  - `portability_clean`
  - `host_bound`
  - `autonomous_members_satisfied`
  - `memory_store_satisfied`
  - `mcp_satisfied`
  - `owner_bridge_present`
  - `resume_placement_mismatch`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `BeginSpawnExecDeniedDigestAbsent`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `placement_present`
  - `portability_clean`
  - `host_bound`
  - `autonomous_members_satisfied`
  - `memory_store_satisfied`
  - `mcp_satisfied`
  - `owner_bridge_present`
  - `resume_placement_consistent`
  - `resolved_spec_digest_absent`
- Emits: `SpawnMemberAdmissionResolved`
- To: `Running`

### `BeginSpawnExecRemote`
- From: `Running`
- On: `BeginSpawnExec`(agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing, placement, workgraph_required, rust_bundles_present, per_spawn_external_tools_present, mob_default_external_tools_present, default_llm_client_override_present, host_surface_mcp_allowlist_present, inherited_tool_filter_present, shell_env_present, mcp_stdio_env_present, mcp_http_headers_present, memory_required, mcp_required, resume_session_id, placed_spawn_id, placed_provision_operation_id, placed_operation_owner_session_id, effective_profile_override_present, effective_model_override_present)
- Guards:
  - `lifecycle_origin_open`
  - `placement_present`
  - `placed_spawn_id_present`
  - `placed_operation_owner_present`
  - `portability_clean`
  - `host_bound`
  - `autonomous_members_satisfied`
  - `memory_store_satisfied`
  - `mcp_satisfied`
  - `owner_bridge_present`
  - `operation_owner_is_owner_bridge`
  - `resume_placement_consistent`
  - `resolved_spec_digest_present`
  - `coordinator_bound`
  - `spawn_exec_settled`
  - `no_pending_placed_spawn`
  - `no_current_placed_spawn`
  - `no_placed_carrier_cleanup`
  - `no_prior_session_binding`
  - `current_binding_absent`
  - `generation_is_next`
  - `replacing_absent`
  - `bridge_session_absent`
  - `identity_not_external_peer`
  - `spawn_profile_name_authorized`
  - `spawn_profile_authorized`
  - `spawn_profile_addressability_authorized`
- Emits: `PersistPendingPlacedSpawn`, `RequestMemberMaterialization`
- To: `Running`

### `RecordMemberMaterializationFailureRunning`
- From: `Running`
- On: `RecordMemberMaterializationFailure`(agent_identity, kind)
- Guards:
  - `spawn_exec_materialize_pending`
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
  - `not_materialize_pending`
- To: `Running`

### `AbortSpawnExecActiveStopped`
- From: `Stopped`
- On: `AbortSpawnExec`(agent_identity)
- Guards:
  - `spawn_exec_in_flight`
  - `not_materialize_pending`
- To: `Stopped`

### `AbortSpawnExecActiveCompleted`
- From: `Completed`
- On: `AbortSpawnExec`(agent_identity)
- Guards:
  - `spawn_exec_in_flight`
  - `not_materialize_pending`
- To: `Completed`

### `AbortSpawnExecMaterializePendingRunning`
- From: `Running`
- On: `AbortSpawnExec`(agent_identity)
- Guards:
  - `spawn_exec_materialize_pending`
- Emits: `PlacedCarrierCleanupRequested`
- To: `Running`

### `AbortSpawnExecMaterializePendingStopped`
- From: `Stopped`
- On: `AbortSpawnExec`(agent_identity)
- Guards:
  - `spawn_exec_materialize_pending`
- Emits: `PlacedCarrierCleanupRequested`
- To: `Stopped`

### `AbortSpawnExecMaterializePendingCompleted`
- From: `Completed`
- On: `AbortSpawnExec`(agent_identity)
- Guards:
  - `spawn_exec_materialize_pending`
- Emits: `PlacedCarrierCleanupRequested`
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

### `AuthorizePlacedCarrierCleanupActiveRunning`
- From: `Running`
- On: `AuthorizePlacedCarrierCleanup`(obligation)
- Guards:
  - `cleanup_pending`
- Emits: `PlacedCarrierCleanupAuthorized`
- To: `Running`

### `AuthorizePlacedCarrierCleanupActiveStopped`
- From: `Stopped`
- On: `AuthorizePlacedCarrierCleanup`(obligation)
- Guards:
  - `cleanup_pending`
- Emits: `PlacedCarrierCleanupAuthorized`
- To: `Stopped`

### `AuthorizePlacedCarrierCleanupActiveCompleted`
- From: `Completed`
- On: `AuthorizePlacedCarrierCleanup`(obligation)
- Guards:
  - `cleanup_pending`
- Emits: `PlacedCarrierCleanupAuthorized`
- To: `Completed`

### `ResolvePlacedCarrierCleanupActiveRunning`
- From: `Running`
- On: `ResolvePlacedCarrierCleanup`(obligation)
- Guards:
  - `cleanup_pending`
- Emits: `PlacedCarrierCleanupResolved`
- To: `Running`

### `ResolvePlacedCarrierCleanupActiveStopped`
- From: `Stopped`
- On: `ResolvePlacedCarrierCleanup`(obligation)
- Guards:
  - `cleanup_pending`
- Emits: `PlacedCarrierCleanupResolved`
- To: `Stopped`

### `ResolvePlacedCarrierCleanupActiveCompleted`
- From: `Completed`
- On: `ResolvePlacedCarrierCleanup`(obligation)
- Guards:
  - `cleanup_pending`
- Emits: `PlacedCarrierCleanupResolved`
- To: `Completed`

### `ResolvePlacedCarrierCleanupLateArrivalRunning`
- From: `Running`
- On: `ResolvePlacedCarrierCleanup`(obligation)
- Guards:
  - `cleanup_absent`
- To: `Running`

### `ResolvePlacedCarrierCleanupLateArrivalStopped`
- From: `Stopped`
- On: `ResolvePlacedCarrierCleanup`(obligation)
- Guards:
  - `cleanup_absent`
- To: `Stopped`

### `ResolvePlacedCarrierCleanupLateArrivalCompleted`
- From: `Completed`
- On: `ResolvePlacedCarrierCleanup`(obligation)
- Guards:
  - `cleanup_absent`
- To: `Completed`

### `AuthorizePlacedCarrierCleanupDestroyed`
- From: `Destroyed`
- On: `AuthorizePlacedCarrierCleanup`(obligation)
- To: `Destroyed`

### `ResolvePlacedCarrierCleanupDestroyed`
- From: `Destroyed`
- On: `ResolvePlacedCarrierCleanup`(obligation)
- To: `Destroyed`

### `AuthorizeSpawnProfileRunning`
- From: `Running`
- On: `AuthorizeSpawnProfile`(agent_identity, profile_name, model, profile_material_digest, tool_config_digest, skills_digest, provider_params_digest, output_schema_digest, external_addressable, resolved_spec_digest)
- Guards:
  - `lifecycle_origin_open`
  - `coordinator_bound`
- Emits: `SpawnProfileAuthorized`
- To: `Running`

### `EnsureMemberRunningExisting`
- From: `Running`
- On: `EnsureMember`(agent_identity)
- Guards:
  - `lifecycle_origin_open`
  - `coordinator_bound`
  - `identity_present`
- Emits: `MemberRetainRequired`
- To: `Running`

### `EnsureMemberRunningMissing`
- From: `Running`
- On: `EnsureMember`(agent_identity)
- Guards:
  - `lifecycle_origin_open`
  - `coordinator_bound`
  - `identity_absent`
- Emits: `MemberSpawnRequired`
- To: `Running`

### `RecoverPendingPlacedSpawnRunning`
- From: `Running`
- On: `RecoverPendingPlacedSpawn`(spawn_id, agent_identity, generation, fence_token, host_id, host_binding_generation, spec_digest, runtime_mode, provision_operation_id, operation_owner_session_id)
- Guards:
  - `owner_bridge_present`
  - `host_binding_generation_positive`
  - `host_binding_generation_observed`
  - `identity_not_live`
  - `pending_attempt_absent`
  - `committed_attempt_absent`
  - `cleanup_absent`
- Emits: `PlacedCarrierCleanupRequested`
- To: `Running`

### `RecoverCommittedPlacedSpawnRunning`
- From: `Running`
- On: `RecoverCommittedPlacedSpawn`(spawn_id, agent_identity, agent_runtime_id, generation, fence_token, host_id, host_binding_generation, member_session_id, member_peer_endpoint, profile_name, runtime_mode, external_addressable, provision_operation_id, operation_owner_session_id)
- Guards:
  - `owner_bridge_present`
  - `operation_owner_is_owner_bridge`
  - `host_binding_generation_positive`
  - `host_binding_generation_observed`
  - `identity_not_recovered`
  - `runtime_not_recovered`
  - `generation_is_next`
  - `pending_attempt_absent`
  - `committed_attempt_absent`
  - `cleanup_absent`
  - `member_peer_id_unowned`
  - `member_peer_id_is_not_host`
- To: `Running`

### `PromoteCommittedPlacedSpawnCarrierBindingRunning`
- From: `Running`
- On: `PromoteCommittedPlacedSpawnCarrierBinding`(spawn_id, agent_identity, generation, fence_token, host_id, expected_host_binding_generation, host_binding_generation)
- Guards:
  - `committed_spawn_exact`
  - `identity_generation_exact`
  - `identity_fence_exact`
  - `placement_host_exact`
  - `expected_carrier_binding_generation_exact`
  - `replacement_binding_generation_advances`
  - `replacement_host_bound`
  - `replacement_binding_generation_exact`
  - `pending_attempt_absent`
  - `cleanup_absent`
  - `placed_kickoff_custody_absent`
- Emits: `PromoteCommittedPlacedSpawnCarrierBinding`
- To: `Running`

### `RecoverPlacedCarrierCleanupRunning`
- From: `Running`
- On: `RecoverPlacedCarrierCleanup`(obligation)
- Guards:
  - `operation_owner_is_owner_bridge`
  - `cleanup_exact_absent`
  - `cleanup_identity_absent`
- Emits: `PlacedCarrierCleanupRequested`
- To: `Running`

### `RecoverPlacedCarrierCleanupAlreadyRunning`
- From: `Running`
- On: `RecoverPlacedCarrierCleanup`(obligation)
- Guards:
  - `operation_owner_is_owner_bridge`
  - `cleanup_exact_present`
- To: `Running`

### `RecoverRosterMemberRunning`
- From: `Running`
- On: `RecoverRosterMember`(agent_identity, agent_runtime_id, fence_token, generation, profile_name, runtime_mode, external_addressable)
- Guards:
  - `identity_not_recovered`
  - `runtime_not_recovered`
  - `generation_is_next`
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
  - `identity_fence_token_matches`
  - `runtime_fence_token_matches`
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
  - `identity_fence_token_matches`
  - `runtime_fence_token_matches`
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
- On: `RecoverRosterMemberReset`(agent_identity, previous_agent_runtime_id, previous_generation, agent_runtime_id, fence_token, generation)
- Guards:
  - `previous_runtime_recovered`
  - `identity_recovered`
  - `previous_generation_matches_history`
  - `generation_is_exact_successor`
  - `fence_token_changes`
  - `replacement_runtime_not_recovered`
  - `previous_runtime_session_ingress_detach_closed`
  - `previous_runtime_not_retiring`
  - `previous_runtime_retirement_closed`
  - `previous_runtime_retire_refusal_closed`
  - `placed_kickoff_custody_absent`
- To: `Running`

### `RecoverRosterMemberRetirementStartedReleasing`
- From: `Running`
- On: `RecoverRosterMemberRetirementStarted`(agent_identity, agent_runtime_id, generation, releasing, session_id, retiring_peer_endpoint, preserve_machine_topology)
- Guards:
  - `placement_absent`
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
- On: `RecoverRosterMemberRetirementStarted`(agent_identity, agent_runtime_id, generation, releasing, session_id, retiring_peer_endpoint, preserve_machine_topology)
- Guards:
  - `placement_absent`
  - `runtime_recovered`
  - `identity_binding_matches`
  - `generation_matches_retired_runtime`
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
- On: `RecoverRosterMemberRetirementStarted`(agent_identity, agent_runtime_id, generation, releasing, session_id, retiring_peer_endpoint, preserve_machine_topology)
- Guards:
  - `placement_absent`
  - `runtime_recovered`
  - `identity_binding_matches`
  - `generation_matches`
  - `releasing_absent`
  - `session_present`
  - `binding_matches_session`
  - `retiring_peer_endpoint_consistent`
- To: `Running`

### `RecoverRosterMemberRetirementStartedPlaced`
- From: `Running`
- On: `RecoverRosterMemberRetirementStarted`(agent_identity, agent_runtime_id, generation, releasing, session_id, retiring_peer_endpoint, preserve_machine_topology)
- Guards:
  - `placement_present`
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
- On: `RecoverRosterMemberRetirementStarted`(agent_identity, agent_runtime_id, generation, releasing, session_id, retiring_peer_endpoint, preserve_machine_topology)
- Guards:
  - `placement_absent`
  - `runtime_recovered`
  - `identity_binding_matches`
  - `generation_matches`
  - `releasing_absent`
  - `session_absent`
  - `binding_absent`
  - `retiring_peer_endpoint_consistent`
- To: `Running`

### `ObserveRespawnTopologyPreservationStartedFresh`
- From: `Running`
- On: `ObserveRespawnTopologyPreservationStarted`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `preservation_not_recorded`
  - `abandonment_not_recorded`
- Emits: `WiringGraphChanged`
- To: `Running`

### `ObserveRespawnTopologyPreservationStartedAlreadyRecorded`
- From: `Running`
- On: `ObserveRespawnTopologyPreservationStarted`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `preservation_recorded`
- To: `Running`

### `ObserveRespawnTopologyPreservationStartedAbandoned`
- From: `Running`
- On: `ObserveRespawnTopologyPreservationStarted`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `preservation_not_recorded`
  - `abandonment_recorded`
- To: `Running`

### `ObserveRespawnTopologyAbandonedRetiringFresh`
- From: `Running`
- On: `ObserveRespawnTopologyAbandoned`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `abandonment_not_recorded`
- Emits: `AppendLifecycleJournal`
- To: `Running`

### `ObserveRespawnTopologyAbandonedRetiringAlreadyRecorded`
- From: `Running`
- On: `ObserveRespawnTopologyAbandoned`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `member_retiring`
  - `preservation_not_pending`
  - `abandonment_recorded`
- To: `Running`

### `ObserveRespawnTopologyAbandonedTerminalFresh`
- From: `Running`
- On: `ObserveRespawnTopologyAbandoned`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_absent`
  - `generation_matches_terminal_history`
  - `fence_token_matches_terminal_history`
  - `preservation_pending`
  - `abandonment_not_recorded`
- Emits: `WiringGraphChanged`, `AppendLifecycleJournal`
- To: `Running`

### `ObserveRespawnTopologyAbandonedTerminalAlreadyRecorded`
- From: `Running`
- On: `ObserveRespawnTopologyAbandoned`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_absent`
  - `generation_matches_terminal_history`
  - `fence_token_matches_terminal_history`
  - `preservation_not_pending`
  - `abandonment_recorded`
- To: `Running`

### `ObserveRespawnTopologyAbandonedTerminalRecoveredFresh`
- From: `Running`
- On: `ObserveRespawnTopologyAbandoned`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_binding_absent`
  - `generation_matches_terminal_history`
  - `fence_token_matches_terminal_history`
  - `preservation_not_pending`
  - `abandonment_not_recorded`
- To: `Running`

### `ObserveRespawnTopologyAbandonedStaleSuccessor`
- From: `Running`
- On: `ObserveRespawnTopologyAbandoned`(agent_identity, agent_runtime_id, fence_token, generation)
- Guards:
  - `identity_rebound`
  - `different_runtime`
  - `successor_generation_differs`
- To: `Running`

### `ObserveRespawnTopologyAbandonedDestroyed`
- From: `Destroyed`
- On: `ObserveRespawnTopologyAbandoned`(agent_identity, agent_runtime_id, fence_token, generation)
- To: `Destroyed`

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
- On: `RecoverRosterMemberRetired`(agent_identity, agent_runtime_id, generation, preserve_machine_topology, preservation_started)
- Guards:
  - `runtime_recovered`
  - `identity_binding_matches`
- To: `Running`

### `RecoverRosterMemberRetiringRunning`
- From: `Running`
- On: `RecoverRosterMemberRetiring`(agent_identity, agent_runtime_id, fence_token, generation, session_id, host_id)
- Guards:
  - `runtime_recovered`
  - `identity_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `session_binding_matches`
  - `placement_matches`
- To: `Running`

### `RecoverRosterMemberRetiringReplay`
- From: `Running`
- On: `RecoverRosterMemberRetiring`(agent_identity, agent_runtime_id, fence_token, generation, session_id, host_id)
- Guards:
  - `already_retiring`
  - `identity_matches`
  - `generation_matches`
  - `fence_token_matches`
  - `session_binding_matches`
  - `placement_matches`
- To: `Running`

### `RecoverRosterMemberRetiredCurrentAlreadyAbsent`
- From: `Running`
- On: `RecoverRosterMemberRetired`(agent_identity, agent_runtime_id, generation, preserve_machine_topology, preservation_started)
- Guards:
  - `runtime_not_recovered`
  - `generation_matches_retired_runtime`
  - `current_binding_absent_or_matches`
- To: `Running`

### `RecoverRosterMemberRetiredStaleGeneration`
- From: `Running`
- On: `RecoverRosterMemberRetired`(agent_identity, agent_runtime_id)
- Guards:
  - `identity_remapped`
- To: `Running`

### `RecoverRosterMemberRetiredStaleIncarnation`
- From: `Running`
- On: `RecoverRosterMemberRetired`(agent_identity, agent_runtime_id, generation)
- Guards:
  - `identity_or_generation_advanced`
- To: `Running`

### `RecoverMemberKickoffPending`
- From: `Running`
- On: `RecoverMemberKickoff`(member_id, phase, error)
- Guards:
  - `placed_kickoff_custody_absent`
  - `placed_kickoff_tombstone_absent`
  - `recover_pending_phase`
  - `recover_pending_without_error`
- To: `Running`

### `RecoverMemberKickoffStarting`
- From: `Running`
- On: `RecoverMemberKickoff`(member_id, phase, error)
- Guards:
  - `placed_kickoff_custody_absent`
  - `placed_kickoff_tombstone_absent`
  - `recover_starting_phase`
  - `recover_starting_without_error`
- To: `Running`

### `RecoverMemberKickoffCallbackPending`
- From: `Running`
- On: `RecoverMemberKickoff`(member_id, phase, error)
- Guards:
  - `placed_kickoff_custody_absent`
  - `placed_kickoff_tombstone_absent`
  - `recover_callback_pending_phase`
  - `recover_callback_pending_without_error`
- To: `Running`

### `RecoverMemberKickoffStarted`
- From: `Running`
- On: `RecoverMemberKickoff`(member_id, phase, error)
- Guards:
  - `placed_kickoff_custody_absent`
  - `placed_kickoff_tombstone_absent`
  - `recover_started_phase`
  - `recover_started_without_error`
- To: `Running`

### `RecoverMemberKickoffFailed`
- From: `Running`
- On: `RecoverMemberKickoff`(member_id, phase, error)
- Guards:
  - `placed_kickoff_custody_absent`
  - `placed_kickoff_tombstone_absent`
  - `recover_failed_phase`
  - `recover_failed_has_error`
- To: `Running`

### `RecoverMemberKickoffCancelled`
- From: `Running`
- On: `RecoverMemberKickoff`(member_id, phase, error)
- Guards:
  - `placed_kickoff_custody_absent`
  - `placed_kickoff_tombstone_absent`
  - `recover_cancelled_phase`
  - `recover_cancelled_without_error`
- To: `Running`

### `RecoverObjectiveBinding`
- From: `Running`
- On: `RecoverObjectiveBinding`(member_id, objective_id)
- Guards:
  - `placed_kickoff_custody_absent_or_exact`
- To: `Running`

### `RecoverFinalizedPlacedKickoffRunning`
- From: `Running`
- On: `RecoverFinalizedPlacedKickoff`(obligation, outcome_kind, outcome_error, closure_kind)
- Guards:
  - `obligation_well_formed`
  - `current_committed_carrier_present`
  - `carrier_host_exact`
  - `carrier_binding_generation_exact`
  - `member_session_exact`
  - `member_generation_exact`
  - `member_fence_exact`
  - `member_autonomous`
  - `objective_exact`
  - `custody_absent`
  - `retained_obligation_absent`
  - `input_unbound`
  - `finalized_chain_matches_public_terminal`
- To: `Running`

### `BindObjectiveOwnerRunning`
- From: `Running`
- On: `BindObjectiveOwner`(owner_id, objective_id)
- Guards:
  - `lifecycle_origin_open`
  - `objective_owner_unbound`
- Emits: `PersistObjectiveOwnerBinding`
- To: `Running`

### `BindObjectiveOwnerIdempotentRunning`
- From: `Running`
- On: `BindObjectiveOwner`(owner_id, objective_id)
- Guards:
  - `objective_owner_matches`
- To: `Running`

### `RecoverObjectiveConclusion`
- From: `Running`
- On: `RecoverObjectiveConclusion`(member_id, objective_id, outcome)
- Guards:
  - `objective_belongs_to_lead`
- To: `Running`

### `ReconcileRunning`
- From: `Running`
- On: `Reconcile`(desired, retire_stale)
- Guards:
  - `lifecycle_origin_open`
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

### `KickoffMarkPending`
- From: `Running`
- On: `KickoffMarkPending`(member_id, objective_id)
- Guards:
  - `lifecycle_origin_open`
  - `placed_kickoff_custody_absent`
  - `kickoff_not_started`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Running`

### `KickoffMarkPendingReplayRunning`
- From: `Running`
- On: `KickoffMarkPending`(member_id, objective_id)
- Guards:
  - `already_pending`
  - `objective_exact`
- To: `Running`

### `KickoffMarkPendingReplayStopped`
- From: `Stopped`
- On: `KickoffMarkPending`(member_id, objective_id)
- Guards:
  - `already_pending`
  - `objective_exact`
- To: `Stopped`

### `KickoffMarkPendingReplayCompleted`
- From: `Completed`
- On: `KickoffMarkPending`(member_id, objective_id)
- Guards:
  - `already_pending`
  - `objective_exact`
- To: `Completed`

### `ConcludeObjectiveRunning`
- From: `Running`
- On: `ConcludeObjective`(member_id, objective_id, outcome)
- Guards:
  - `objective_belongs_to_lead`
  - `objective_not_concluded`
- Emits: `PersistObjectiveConclusion`
- To: `Running`

### `ConcludeObjectiveIdempotentRunning`
- From: `Running`
- On: `ConcludeObjective`(member_id, objective_id, outcome)
- Guards:
  - `objective_belongs_to_lead`
  - `objective_already_concluded`
  - `objective_outcome_matches`
- To: `Running`

### `KickoffMarkStarting`
- From: `Running`
- On: `KickoffMarkStarting`(member_id)
- Guards:
  - `lifecycle_origin_open`
  - `kickoff_pending`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Running`

### `KickoffMarkStartingReplayRunning`
- From: `Running`
- On: `KickoffMarkStarting`(member_id)
- Guards:
  - `already_starting`
- To: `Running`

### `KickoffMarkStartingReplayStopped`
- From: `Stopped`
- On: `KickoffMarkStarting`(member_id)
- Guards:
  - `already_starting`
- To: `Stopped`

### `KickoffMarkStartingReplayCompleted`
- From: `Completed`
- On: `KickoffMarkStarting`(member_id)
- Guards:
  - `already_starting`
- To: `Completed`

### `StartPlacedKickoffFresh`
- From: `Running`
- On: `StartPlacedKickoff`(obligation)
- Guards:
  - `lifecycle_origin_open`
  - `obligation_well_formed`
  - `target_placed`
  - `target_host_bound`
  - `target_carrier_binding_active`
  - `target_host_binding_generation_exact`
  - `target_host_supports_tracked_turns`
  - `target_session_exact`
  - `target_runtime_present`
  - `target_runtime_live`
  - `target_not_retiring`
  - `target_autonomous`
  - `target_spawn_settled`
  - `target_not_reviving`
  - `target_materialization_healthy`
  - `target_carrier_cleanup_absent`
  - `current_generation`
  - `current_fence`
  - `kickoff_pending`
  - `objective_exact`
  - `input_unbound`
  - `correlation_available`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Running`

### `StartPlacedKickoffPendingReplayRunning`
- From: `Running`
- On: `StartPlacedKickoff`(obligation)
- Guards:
  - `already_pending`
  - `retained_obligation_exact`
  - `input_exact`
  - `kickoff_active_or_cancelled`
- To: `Running`

### `StartPlacedKickoffPendingReplayStopped`
- From: `Stopped`
- On: `StartPlacedKickoff`(obligation)
- Guards:
  - `already_pending`
  - `retained_obligation_exact`
  - `input_exact`
  - `kickoff_active_or_cancelled`
- To: `Stopped`

### `StartPlacedKickoffPendingReplayCompleted`
- From: `Completed`
- On: `StartPlacedKickoff`(obligation)
- Guards:
  - `already_pending`
  - `retained_obligation_exact`
  - `input_exact`
  - `kickoff_active_or_cancelled`
- To: `Completed`

### `StartPlacedKickoffResolvedReplayRunning`
- From: `Running`
- On: `StartPlacedKickoff`(obligation)
- Guards:
  - `already_resolved`
  - `retained_obligation_exact`
  - `input_exact`
- To: `Running`

### `StartPlacedKickoffResolvedReplayStopped`
- From: `Stopped`
- On: `StartPlacedKickoff`(obligation)
- Guards:
  - `already_resolved`
  - `retained_obligation_exact`
  - `input_exact`
- To: `Stopped`

### `StartPlacedKickoffResolvedReplayCompleted`
- From: `Completed`
- On: `StartPlacedKickoff`(obligation)
- Guards:
  - `already_resolved`
  - `retained_obligation_exact`
  - `input_exact`
- To: `Completed`

### `StartPlacedKickoffFinalReplayRunning`
- From: `Running`
- On: `StartPlacedKickoff`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `retained_terminal_closure`
  - `kickoff_terminal`
- To: `Running`

### `StartPlacedKickoffFinalReplayStopped`
- From: `Stopped`
- On: `StartPlacedKickoff`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `retained_terminal_closure`
  - `kickoff_terminal`
- To: `Stopped`

### `StartPlacedKickoffFinalReplayCompleted`
- From: `Completed`
- On: `StartPlacedKickoff`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `retained_terminal_closure`
  - `kickoff_terminal`
- To: `Completed`

### `ResolvePlacedKickoffStartedFreshRunning`
- From: `Running`
- On: `ResolvePlacedKickoffStarted`(obligation)
- Guards:
  - `pending`
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Running`

### `ResolvePlacedKickoffStartedFreshStopped`
- From: `Stopped`
- On: `ResolvePlacedKickoffStarted`(obligation)
- Guards:
  - `pending`
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Stopped`

### `ResolvePlacedKickoffStartedFreshCompleted`
- From: `Completed`
- On: `ResolvePlacedKickoffStarted`(obligation)
- Guards:
  - `pending`
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Completed`

### `ResolvePlacedKickoffStartedAfterCancellationRunning`
- From: `Running`
- On: `ResolvePlacedKickoffStarted`(obligation)
- Guards:
  - `pending`
  - `kickoff_cancelled`
- To: `Running`

### `ResolvePlacedKickoffStartedAfterCancellationStopped`
- From: `Stopped`
- On: `ResolvePlacedKickoffStarted`(obligation)
- Guards:
  - `pending`
  - `kickoff_cancelled`
- To: `Stopped`

### `ResolvePlacedKickoffStartedAfterCancellationCompleted`
- From: `Completed`
- On: `ResolvePlacedKickoffStarted`(obligation)
- Guards:
  - `pending`
  - `kickoff_cancelled`
- To: `Completed`

### `ResolvePlacedKickoffStartedReplayRunning`
- From: `Running`
- On: `ResolvePlacedKickoffStarted`(obligation)
- Guards:
  - `resolved`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Running`

### `ResolvePlacedKickoffStartedReplayStopped`
- From: `Stopped`
- On: `ResolvePlacedKickoffStarted`(obligation)
- Guards:
  - `resolved`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Stopped`

### `ResolvePlacedKickoffStartedReplayCompleted`
- From: `Completed`
- On: `ResolvePlacedKickoffStarted`(obligation)
- Guards:
  - `resolved`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Completed`

### `ResolvePlacedKickoffStartedFinalReplayRunning`
- From: `Running`
- On: `ResolvePlacedKickoffStarted`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Running`

### `ResolvePlacedKickoffStartedFinalReplayStopped`
- From: `Stopped`
- On: `ResolvePlacedKickoffStarted`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Stopped`

### `ResolvePlacedKickoffStartedFinalReplayCompleted`
- From: `Completed`
- On: `ResolvePlacedKickoffStarted`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Completed`

### `ResolvePlacedKickoffStartedFinalReplayDestroyed`
- From: `Destroyed`
- On: `ResolvePlacedKickoffStarted`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Destroyed`

### `ResolvePlacedKickoffCallbackPendingFreshRunning`
- From: `Running`
- On: `ResolvePlacedKickoffCallbackPending`(obligation)
- Guards:
  - `pending`
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Running`

### `ResolvePlacedKickoffCallbackPendingFreshStopped`
- From: `Stopped`
- On: `ResolvePlacedKickoffCallbackPending`(obligation)
- Guards:
  - `pending`
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Stopped`

### `ResolvePlacedKickoffCallbackPendingFreshCompleted`
- From: `Completed`
- On: `ResolvePlacedKickoffCallbackPending`(obligation)
- Guards:
  - `pending`
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Completed`

### `ResolvePlacedKickoffCallbackPendingAfterCancellationRunning`
- From: `Running`
- On: `ResolvePlacedKickoffCallbackPending`(obligation)
- Guards:
  - `pending`
  - `kickoff_cancelled`
- To: `Running`

### `ResolvePlacedKickoffCallbackPendingAfterCancellationStopped`
- From: `Stopped`
- On: `ResolvePlacedKickoffCallbackPending`(obligation)
- Guards:
  - `pending`
  - `kickoff_cancelled`
- To: `Stopped`

### `ResolvePlacedKickoffCallbackPendingAfterCancellationCompleted`
- From: `Completed`
- On: `ResolvePlacedKickoffCallbackPending`(obligation)
- Guards:
  - `pending`
  - `kickoff_cancelled`
- To: `Completed`

### `ResolvePlacedKickoffCallbackPendingReplayRunning`
- From: `Running`
- On: `ResolvePlacedKickoffCallbackPending`(obligation)
- Guards:
  - `resolved`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Running`

### `ResolvePlacedKickoffCallbackPendingReplayStopped`
- From: `Stopped`
- On: `ResolvePlacedKickoffCallbackPending`(obligation)
- Guards:
  - `resolved`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Stopped`

### `ResolvePlacedKickoffCallbackPendingReplayCompleted`
- From: `Completed`
- On: `ResolvePlacedKickoffCallbackPending`(obligation)
- Guards:
  - `resolved`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Completed`

### `ResolvePlacedKickoffCallbackPendingFinalReplayRunning`
- From: `Running`
- On: `ResolvePlacedKickoffCallbackPending`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Running`

### `ResolvePlacedKickoffCallbackPendingFinalReplayStopped`
- From: `Stopped`
- On: `ResolvePlacedKickoffCallbackPending`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Stopped`

### `ResolvePlacedKickoffCallbackPendingFinalReplayCompleted`
- From: `Completed`
- On: `ResolvePlacedKickoffCallbackPending`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Completed`

### `ResolvePlacedKickoffCallbackPendingFinalReplayDestroyed`
- From: `Destroyed`
- On: `ResolvePlacedKickoffCallbackPending`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Destroyed`

### `ResolvePlacedKickoffFailedFreshRunning`
- From: `Running`
- On: `ResolvePlacedKickoffFailed`(obligation, error)
- Guards:
  - `pending`
  - `kickoff_starting`
- Emits: `PersistKickoffFailureUpdate`, `EmitKickoffLifecycleNotice`
- To: `Running`

### `ResolvePlacedKickoffFailedFreshStopped`
- From: `Stopped`
- On: `ResolvePlacedKickoffFailed`(obligation, error)
- Guards:
  - `pending`
  - `kickoff_starting`
- Emits: `PersistKickoffFailureUpdate`, `EmitKickoffLifecycleNotice`
- To: `Stopped`

### `ResolvePlacedKickoffFailedFreshCompleted`
- From: `Completed`
- On: `ResolvePlacedKickoffFailed`(obligation, error)
- Guards:
  - `pending`
  - `kickoff_starting`
- Emits: `PersistKickoffFailureUpdate`, `EmitKickoffLifecycleNotice`
- To: `Completed`

### `ResolvePlacedKickoffFailedAfterCancellationRunning`
- From: `Running`
- On: `ResolvePlacedKickoffFailed`(obligation, error)
- Guards:
  - `pending`
  - `kickoff_cancelled`
- To: `Running`

### `ResolvePlacedKickoffFailedAfterCancellationStopped`
- From: `Stopped`
- On: `ResolvePlacedKickoffFailed`(obligation, error)
- Guards:
  - `pending`
  - `kickoff_cancelled`
- To: `Stopped`

### `ResolvePlacedKickoffFailedAfterCancellationCompleted`
- From: `Completed`
- On: `ResolvePlacedKickoffFailed`(obligation, error)
- Guards:
  - `pending`
  - `kickoff_cancelled`
- To: `Completed`

### `ResolvePlacedKickoffFailedReplayRunning`
- From: `Running`
- On: `ResolvePlacedKickoffFailed`(obligation, error)
- Guards:
  - `resolved`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Running`

### `ResolvePlacedKickoffFailedReplayStopped`
- From: `Stopped`
- On: `ResolvePlacedKickoffFailed`(obligation, error)
- Guards:
  - `resolved`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Stopped`

### `ResolvePlacedKickoffFailedReplayCompleted`
- From: `Completed`
- On: `ResolvePlacedKickoffFailed`(obligation, error)
- Guards:
  - `resolved`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Completed`

### `ResolvePlacedKickoffFailedFinalReplayRunning`
- From: `Running`
- On: `ResolvePlacedKickoffFailed`(obligation, error)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Running`

### `ResolvePlacedKickoffFailedFinalReplayStopped`
- From: `Stopped`
- On: `ResolvePlacedKickoffFailed`(obligation, error)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Stopped`

### `ResolvePlacedKickoffFailedFinalReplayCompleted`
- From: `Completed`
- On: `ResolvePlacedKickoffFailed`(obligation, error)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Completed`

### `ResolvePlacedKickoffFailedFinalReplayDestroyed`
- From: `Destroyed`
- On: `ResolvePlacedKickoffFailed`(obligation, error)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Destroyed`

### `ResolvePlacedKickoffCancelledFreshRunning`
- From: `Running`
- On: `ResolvePlacedKickoffCancelled`(obligation)
- Guards:
  - `pending`
  - `kickoff_starting_or_cancelled`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Running`

### `ResolvePlacedKickoffCancelledFreshStopped`
- From: `Stopped`
- On: `ResolvePlacedKickoffCancelled`(obligation)
- Guards:
  - `pending`
  - `kickoff_starting_or_cancelled`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Stopped`

### `ResolvePlacedKickoffCancelledFreshCompleted`
- From: `Completed`
- On: `ResolvePlacedKickoffCancelled`(obligation)
- Guards:
  - `pending`
  - `kickoff_starting_or_cancelled`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Completed`

### `ResolvePlacedKickoffCancelledReplayRunning`
- From: `Running`
- On: `ResolvePlacedKickoffCancelled`(obligation)
- Guards:
  - `resolved`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Running`

### `ResolvePlacedKickoffCancelledReplayStopped`
- From: `Stopped`
- On: `ResolvePlacedKickoffCancelled`(obligation)
- Guards:
  - `resolved`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Stopped`

### `ResolvePlacedKickoffCancelledReplayCompleted`
- From: `Completed`
- On: `ResolvePlacedKickoffCancelled`(obligation)
- Guards:
  - `resolved`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Completed`

### `ResolvePlacedKickoffCancelledFinalReplayRunning`
- From: `Running`
- On: `ResolvePlacedKickoffCancelled`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Running`

### `ResolvePlacedKickoffCancelledFinalReplayStopped`
- From: `Stopped`
- On: `ResolvePlacedKickoffCancelled`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Stopped`

### `ResolvePlacedKickoffCancelledFinalReplayCompleted`
- From: `Completed`
- On: `ResolvePlacedKickoffCancelled`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Completed`

### `ResolvePlacedKickoffCancelledFinalReplayDestroyed`
- From: `Destroyed`
- On: `ResolvePlacedKickoffCancelled`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `host_outcome_exact`
- To: `Destroyed`

### `RejectPlacedKickoffBeforeAdmissionFreshRunning`
- From: `Running`
- On: `RejectPlacedKickoffBeforeAdmission`(obligation, error)
- Guards:
  - `pending`
  - `kickoff_starting`
  - `error_nonempty`
- Emits: `PersistKickoffFailureUpdate`, `EmitKickoffLifecycleNotice`
- To: `Running`

### `RejectPlacedKickoffBeforeAdmissionFreshStopped`
- From: `Stopped`
- On: `RejectPlacedKickoffBeforeAdmission`(obligation, error)
- Guards:
  - `pending`
  - `kickoff_starting`
  - `error_nonempty`
- Emits: `PersistKickoffFailureUpdate`, `EmitKickoffLifecycleNotice`
- To: `Stopped`

### `RejectPlacedKickoffBeforeAdmissionFreshCompleted`
- From: `Completed`
- On: `RejectPlacedKickoffBeforeAdmission`(obligation, error)
- Guards:
  - `pending`
  - `kickoff_starting`
  - `error_nonempty`
- Emits: `PersistKickoffFailureUpdate`, `EmitKickoffLifecycleNotice`
- To: `Completed`

### `RejectPlacedKickoffBeforeAdmissionAfterCancellationRunning`
- From: `Running`
- On: `RejectPlacedKickoffBeforeAdmission`(obligation, error)
- Guards:
  - `pending`
  - `kickoff_cancelled`
  - `error_nonempty`
- To: `Running`

### `RejectPlacedKickoffBeforeAdmissionAfterCancellationStopped`
- From: `Stopped`
- On: `RejectPlacedKickoffBeforeAdmission`(obligation, error)
- Guards:
  - `pending`
  - `kickoff_cancelled`
  - `error_nonempty`
- To: `Stopped`

### `RejectPlacedKickoffBeforeAdmissionAfterCancellationCompleted`
- From: `Completed`
- On: `RejectPlacedKickoffBeforeAdmission`(obligation, error)
- Guards:
  - `pending`
  - `kickoff_cancelled`
  - `error_nonempty`
- To: `Completed`

### `RejectPlacedKickoffBeforeAdmissionReplayRunning`
- From: `Running`
- On: `RejectPlacedKickoffBeforeAdmission`(obligation, error)
- Guards:
  - `error_nonempty`
  - `custody_absent`
  - `retained_obligation_exact`
  - `no_effect_outcome_exact`
  - `closure_exact`
  - `terminal_matches`
- To: `Running`

### `RejectPlacedKickoffBeforeAdmissionReplayStopped`
- From: `Stopped`
- On: `RejectPlacedKickoffBeforeAdmission`(obligation, error)
- Guards:
  - `error_nonempty`
  - `custody_absent`
  - `retained_obligation_exact`
  - `no_effect_outcome_exact`
  - `closure_exact`
  - `terminal_matches`
- To: `Stopped`

### `RejectPlacedKickoffBeforeAdmissionReplayCompleted`
- From: `Completed`
- On: `RejectPlacedKickoffBeforeAdmission`(obligation, error)
- Guards:
  - `error_nonempty`
  - `custody_absent`
  - `retained_obligation_exact`
  - `no_effect_outcome_exact`
  - `closure_exact`
  - `terminal_matches`
- To: `Completed`

### `AcknowledgePlacedKickoffOutcomePresentRunning`
- From: `Running`
- On: `AcknowledgePlacedKickoffOutcome`(obligation)
- Guards:
  - `resolved`
  - `retained_obligation_exact`
- To: `Running`

### `AcknowledgePlacedKickoffOutcomePresentStopped`
- From: `Stopped`
- On: `AcknowledgePlacedKickoffOutcome`(obligation)
- Guards:
  - `resolved`
  - `retained_obligation_exact`
- To: `Stopped`

### `AcknowledgePlacedKickoffOutcomePresentCompleted`
- From: `Completed`
- On: `AcknowledgePlacedKickoffOutcome`(obligation)
- Guards:
  - `resolved`
  - `retained_obligation_exact`
- To: `Completed`

### `AcknowledgePlacedKickoffOutcomePresentDestroyed`
- From: `Destroyed`
- On: `AcknowledgePlacedKickoffOutcome`(obligation)
- Guards:
  - `resolved`
  - `retained_obligation_exact`
- To: `Destroyed`

### `AcknowledgePlacedKickoffOutcomeReplayRunning`
- From: `Running`
- On: `AcknowledgePlacedKickoffOutcome`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `closure_exact`
- To: `Running`

### `AcknowledgePlacedKickoffOutcomeReplayStopped`
- From: `Stopped`
- On: `AcknowledgePlacedKickoffOutcome`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `closure_exact`
- To: `Stopped`

### `AcknowledgePlacedKickoffOutcomeReplayCompleted`
- From: `Completed`
- On: `AcknowledgePlacedKickoffOutcome`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `closure_exact`
- To: `Completed`

### `AcknowledgePlacedKickoffOutcomeReplayDestroyed`
- From: `Destroyed`
- On: `AcknowledgePlacedKickoffOutcome`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `closure_exact`
- To: `Destroyed`

### `DisposePlacedKickoffObligationActiveRunning`
- From: `Running`
- On: `DisposePlacedKickoffObligation`(obligation)
- Guards:
  - `custody_present`
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Running`

### `DisposePlacedKickoffObligationActiveStopped`
- From: `Stopped`
- On: `DisposePlacedKickoffObligation`(obligation)
- Guards:
  - `custody_present`
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Stopped`

### `DisposePlacedKickoffObligationActiveCompleted`
- From: `Completed`
- On: `DisposePlacedKickoffObligation`(obligation)
- Guards:
  - `custody_present`
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Completed`

### `DisposePlacedKickoffObligationActiveDestroyed`
- From: `Destroyed`
- On: `DisposePlacedKickoffObligation`(obligation)
- Guards:
  - `custody_present`
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Destroyed`

### `DisposePlacedKickoffObligationTerminalRunning`
- From: `Running`
- On: `DisposePlacedKickoffObligation`(obligation)
- Guards:
  - `custody_present`
  - `kickoff_not_starting`
- To: `Running`

### `DisposePlacedKickoffObligationTerminalStopped`
- From: `Stopped`
- On: `DisposePlacedKickoffObligation`(obligation)
- Guards:
  - `custody_present`
  - `kickoff_not_starting`
- To: `Stopped`

### `DisposePlacedKickoffObligationTerminalCompleted`
- From: `Completed`
- On: `DisposePlacedKickoffObligation`(obligation)
- Guards:
  - `custody_present`
  - `kickoff_not_starting`
- To: `Completed`

### `DisposePlacedKickoffObligationTerminalDestroyed`
- From: `Destroyed`
- On: `DisposePlacedKickoffObligation`(obligation)
- Guards:
  - `custody_present`
  - `kickoff_not_starting`
- To: `Destroyed`

### `DisposePlacedKickoffObligationReplayRunning`
- From: `Running`
- On: `DisposePlacedKickoffObligation`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `closure_exact`
- To: `Running`

### `DisposePlacedKickoffObligationReplayStopped`
- From: `Stopped`
- On: `DisposePlacedKickoffObligation`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `closure_exact`
- To: `Stopped`

### `DisposePlacedKickoffObligationReplayCompleted`
- From: `Completed`
- On: `DisposePlacedKickoffObligation`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `closure_exact`
- To: `Completed`

### `DisposePlacedKickoffObligationReplayDestroyed`
- From: `Destroyed`
- On: `DisposePlacedKickoffObligation`(obligation)
- Guards:
  - `custody_absent`
  - `retained_obligation_exact`
  - `closure_exact`
- To: `Destroyed`

### `KickoffResolveStartedRunning`
- From: `Running`
- On: `KickoffResolveStarted`(member_id)
- Guards:
  - `placed_kickoff_custody_absent`
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Running`

### `KickoffResolveStartedStopped`
- From: `Stopped`
- On: `KickoffResolveStarted`(member_id)
- Guards:
  - `placed_kickoff_custody_absent`
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Stopped`

### `KickoffResolveStartedCompleted`
- From: `Completed`
- On: `KickoffResolveStarted`(member_id)
- Guards:
  - `placed_kickoff_custody_absent`
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Completed`

### `KickoffResolveCallbackPendingRunning`
- From: `Running`
- On: `KickoffResolveCallbackPending`(member_id)
- Guards:
  - `placed_kickoff_custody_absent`
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Running`

### `KickoffResolveCallbackPendingStopped`
- From: `Stopped`
- On: `KickoffResolveCallbackPending`(member_id)
- Guards:
  - `placed_kickoff_custody_absent`
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Stopped`

### `KickoffResolveCallbackPendingCompleted`
- From: `Completed`
- On: `KickoffResolveCallbackPending`(member_id)
- Guards:
  - `placed_kickoff_custody_absent`
  - `kickoff_starting`
- Emits: `PersistKickoffUpdate`, `EmitKickoffLifecycleNotice`
- To: `Completed`

### `KickoffResolveFailedFromStartingRunning`
- From: `Running`
- On: `KickoffResolveFailed`(member_id, error)
- Guards:
  - `placed_kickoff_custody_absent`
  - `kickoff_active_failed`
- Emits: `PersistKickoffFailureUpdate`, `EmitKickoffLifecycleNotice`
- To: `Running`

### `KickoffResolveFailedFromStartingStopped`
- From: `Stopped`
- On: `KickoffResolveFailed`(member_id, error)
- Guards:
  - `placed_kickoff_custody_absent`
  - `kickoff_active_failed`
- Emits: `PersistKickoffFailureUpdate`, `EmitKickoffLifecycleNotice`
- To: `Stopped`

### `KickoffResolveFailedFromStartingCompleted`
- From: `Completed`
- On: `KickoffResolveFailed`(member_id, error)
- Guards:
  - `placed_kickoff_custody_absent`
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
- Guards:
  - `placed_kickoff_custody_absent`
- To: `Running`

### `KickoffClearStopped`
- From: `Stopped`
- On: `KickoffClear`(member_id)
- Guards:
  - `placed_kickoff_custody_absent`
- To: `Stopped`

### `KickoffClearCompleted`
- From: `Completed`
- On: `KickoffClear`(member_id)
- Guards:
  - `placed_kickoff_custody_absent`
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
- Guards:
  - `generation_space_available`
- Emits: `RespawnGenerationComputed`
- To: `Running`

### `ComputeRespawnGenerationStopped`
- From: `Stopped`
- On: `ComputeRespawnGeneration`(agent_identity)
- Guards:
  - `generation_space_available`
- Emits: `RespawnGenerationComputed`
- To: `Stopped`

### `ComputeRespawnGenerationCompleted`
- From: `Completed`
- On: `ComputeRespawnGeneration`(agent_identity)
- Guards:
  - `generation_space_available`
- Emits: `RespawnGenerationComputed`
- To: `Completed`

### `ComputeRespawnGenerationDestroyed`
- From: `Destroyed`
- On: `ComputeRespawnGeneration`(agent_identity)
- Guards:
  - `generation_space_available`
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
- Guards:
  - `lifecycle_origin_open`
- Emits: `SupervisorEscalationRequested`
- To: `Running`

### `EscalateToSupervisorTargetFoundStopped`
- From: `Stopped`
- On: `EscalateToSupervisor`(run_id, step_id, supervisor_identity, turn_timeout_ms)
- Guards:
  - `lifecycle_origin_open`
- Emits: `SupervisorEscalationRequested`
- To: `Stopped`

### `EscalateToSupervisorTargetFoundCompleted`
- From: `Completed`
- On: `EscalateToSupervisor`(run_id, step_id, supervisor_identity, turn_timeout_ms)
- Guards:
  - `lifecycle_origin_open`
- Emits: `SupervisorEscalationRequested`
- To: `Completed`

### `EscalateToSupervisorTargetFoundDestroyed`
- From: `Destroyed`
- On: `EscalateToSupervisor`(run_id, step_id, supervisor_identity, turn_timeout_ms)
- Guards:
  - `lifecycle_origin_open`
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
  - `placed_completion_origin_open`
  - `active_members_present`
  - `identity_binding_matches`
  - `current_binding_matches`
  - `fence_token_matches`
  - `generation_binding_present`
  - `session_binding_present`
  - `placed_carrier_binding_active_or_local`
  - `member_not_retiring`
  - `external_origin`
  - `runtime_externally_addressable`
- Emits: `RequestRuntimeIngress`
- To: `Running`

### `SubmitWorkRunningExternalPeerOnly`
- From: `Running`
- On: `SubmitWork`(agent_identity, agent_runtime_id, fence_token, work_id, origin)
- Guards:
  - `placed_completion_origin_open`
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
  - `placed_completion_origin_open`
  - `active_members_present`
  - `identity_binding_matches`
  - `current_binding_matches`
  - `fence_token_matches`
  - `generation_binding_present`
  - `session_binding_present`
  - `placed_carrier_binding_active_or_local`
  - `member_not_retiring`
  - `internal_origin`
- Emits: `RequestRuntimeIngress`
- To: `Running`

### `SubmitWorkRunningInternalPeerOnly`
- From: `Running`
- On: `SubmitWork`(agent_identity, agent_runtime_id, fence_token, work_id, origin)
- Guards:
  - `placed_completion_origin_open`
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

### `RetireMember`
- From: `Running`
- On: `RetireMember`(agent_identity, agent_runtime_id, fence_token, session_id)
- Guards:
  - `placement_absent`
  - `identity_binding_matches`
  - `current_binding_matches`
  - `fence_token_matches`
  - `member_not_retiring`
  - `retirement_session_absent`
  - `retire_refusal_absent`
  - `session_present`
  - `session_binding_matches`
- Emits: `AppendLifecycleJournal`, `RequestRuntimeRetire`, `RequestKickoffQuiesce`
- To: `Running`

### `RetireMemberRemote`
- From: `Running`
- On: `RetireMember`(agent_identity, agent_runtime_id, fence_token, session_id)
- Guards:
  - `placement_present`
  - `placed_carrier_binding_active`
  - `identity_binding_matches`
  - `current_binding_matches`
  - `fence_token_matches`
  - `member_not_retiring`
  - `session_present`
  - `session_binding_matches`
- Emits: `AppendLifecycleJournal`, `RequestMemberRelease`, `RequestKickoffQuiesce`
- To: `Running`

### `RetireMemberRemoteConfirmedRevoked`
- From: `Running`
- On: `RetireMember`(agent_identity, agent_runtime_id, fence_token, session_id)
- Guards:
  - `placement_present`
  - `placed_carrier_binding_confirmed_revoked`
  - `identity_binding_matches`
  - `current_binding_matches`
  - `fence_token_matches`
  - `member_not_retiring`
  - `session_present`
  - `session_binding_matches`
- Emits: `AppendLifecycleJournal`, `RequestKickoffQuiesce`
- To: `Running`

### `RetireMemberPeerOnly`
- From: `Running`
- On: `RetireMember`(agent_identity, agent_runtime_id, fence_token, session_id)
- Guards:
  - `placement_absent`
  - `identity_binding_matches`
  - `current_binding_matches`
  - `fence_token_matches`
  - `member_not_retiring`
  - `session_absent`
  - `session_binding_absent`
- Emits: `AppendLifecycleJournal`, `RequestKickoffQuiesce`
- To: `Running`

### `ResolveRuntimeBindingRefusalRunning`
- From: `Running`
- On: `ResolveRuntimeBindingRefusal`(agent_identity, agent_runtime_id, session_id, refusal_code, reason)
- Guards:
  - `placement_absent`
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
  - `placement_absent`
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
  - `placement_absent`
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

### `AdmitDestroyMemberRetireRemoteRunning`
- From: `Running`
- On: `AdmitDestroyMemberRetire`(mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `destroy_admitted`
  - `placement_present`
  - `placed_carrier_binding_active`
  - `identity_binding_matches`
  - `current_binding_matches`
  - `fence_token_matches`
  - `generation_matches`
  - `session_present`
  - `session_binding_matches`
- Emits: `AppendLifecycleJournal`, `RequestMemberRelease`, `RequestKickoffQuiesce`
- To: `Running`

### `AdmitDestroyMemberRetireRemoteStopped`
- From: `Stopped`
- On: `AdmitDestroyMemberRetire`(mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `destroy_admitted`
  - `placement_present`
  - `placed_carrier_binding_active`
  - `identity_binding_matches`
  - `current_binding_matches`
  - `fence_token_matches`
  - `generation_matches`
  - `session_present`
  - `session_binding_matches`
- Emits: `AppendLifecycleJournal`, `RequestMemberRelease`, `RequestKickoffQuiesce`
- To: `Stopped`

### `AdmitDestroyMemberRetireConfirmedRevokedRunning`
- From: `Running`
- On: `AdmitDestroyMemberRetire`(mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `destroy_admitted`
  - `placement_present`
  - `placed_carrier_binding_confirmed_revoked`
  - `identity_binding_matches`
  - `current_binding_matches`
  - `fence_token_matches`
  - `generation_matches`
  - `session_present`
  - `session_binding_matches`
- Emits: `AppendLifecycleJournal`, `RequestKickoffQuiesce`
- To: `Running`

### `AdmitDestroyMemberRetireConfirmedRevokedStopped`
- From: `Stopped`
- On: `AdmitDestroyMemberRetire`(mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `destroy_admitted`
  - `placement_present`
  - `placed_carrier_binding_confirmed_revoked`
  - `identity_binding_matches`
  - `current_binding_matches`
  - `fence_token_matches`
  - `generation_matches`
  - `session_present`
  - `session_binding_matches`
- Emits: `AppendLifecycleJournal`, `RequestKickoffQuiesce`
- To: `Stopped`

### `AdmitDestroyMemberRetirePeerOnlyRunning`
- From: `Running`
- On: `AdmitDestroyMemberRetire`(mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id)
- Guards:
  - `destroy_admitted`
  - `placement_absent`
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
  - `placement_absent`
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
  - `placement_absent`
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
  - `placement_absent`
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
  - `placement_absent`
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
  - `placement_absent`
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
  - `placement_absent`
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
  - `placement_absent`
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
  - `identity_binding_cleared`
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
  - `identity_binding_cleared`
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
- On: `ObserveRemoteMemberRetirementArchivedAndSupervisorRevoked`(agent_identity, agent_runtime_id, fence_token, generation, preserve_machine_topology)
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
- On: `ObserveRemoteMemberRetirementArchivedAndSupervisorRevoked`(agent_identity, agent_runtime_id, fence_token, generation, preserve_machine_topology)
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
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal, preserve_machine_topology)
- Guards:
  - `topology_not_preserved`
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `placed_carrier_absent`
  - `runtime_live`
  - `fence_token_matches`
  - `generation_matches`
  - `member_retiring`
  - `retirement_session_matches`
  - `session_present`
  - `session_binding_matches_or_cleared`
  - `kickoff_quiesced`
- Emits: `AppendLifecycleJournal`, `WiringGraphChanged`, `EmitMemberLifecycleNotice`
- To: `Running`

### `ObserveMemberRetirementArchivedLivePreservingTopology`
- From: `Running`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal, preserve_machine_topology)
- Guards:
  - `topology_preserved`
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `placed_carrier_absent`
  - `runtime_live`
  - `fence_token_matches`
  - `generation_matches`
  - `member_retiring`
  - `retirement_session_matches`
  - `session_present`
  - `session_binding_matches_or_cleared`
  - `kickoff_quiesced`
- Emits: `AppendLifecycleJournal`, `EmitMemberLifecycleNotice`
- To: `Running`

### `ObserveMemberRetirementArchivedLiveStopped`
- From: `Stopped`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal, preserve_machine_topology)
- Guards:
  - `topology_not_preserved`
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `placed_carrier_absent`
  - `runtime_live`
  - `fence_token_matches`
  - `generation_matches`
  - `member_retiring`
  - `retirement_session_matches`
  - `session_present`
  - `session_binding_matches_or_cleared`
  - `kickoff_quiesced`
- Emits: `AppendLifecycleJournal`, `WiringGraphChanged`, `EmitMemberLifecycleNotice`
- To: `Stopped`

### `ObserveMemberRetirementArchivedRetired`
- From: `Running`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal, preserve_machine_topology)
- Guards:
  - `topology_not_preserved`
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `placed_carrier_absent`
  - `runtime_not_live`
  - `member_retiring`
  - `generation_matches`
  - `fence_token_matches_history`
  - `retirement_session_matches`
  - `session_present`
  - `session_binding_matches_or_cleared`
  - `kickoff_quiesced`
- Emits: `AppendLifecycleJournal`, `WiringGraphChanged`
- To: `Running`

### `ObserveMemberRetirementArchivedRetiredPreservingTopology`
- From: `Running`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal, preserve_machine_topology)
- Guards:
  - `topology_preserved`
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `placed_carrier_absent`
  - `runtime_not_live`
  - `member_retiring`
  - `generation_matches`
  - `fence_token_matches_history`
  - `retirement_session_matches`
  - `session_present`
  - `session_binding_matches_or_cleared`
  - `kickoff_quiesced`
- Emits: `AppendLifecycleJournal`
- To: `Running`

### `ObserveMemberRetirementArchivedRetiredStopped`
- From: `Stopped`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal, preserve_machine_topology)
- Guards:
  - `topology_not_preserved`
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `placed_carrier_absent`
  - `runtime_not_live`
  - `member_retiring`
  - `generation_matches`
  - `fence_token_matches_history`
  - `retirement_session_matches`
  - `session_present`
  - `session_binding_matches_or_cleared`
  - `kickoff_quiesced`
- Emits: `AppendLifecycleJournal`, `WiringGraphChanged`
- To: `Stopped`

### `ObserveMemberRetirementArchivedPlacedLiveRunning`
- From: `Running`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal, preserve_machine_topology)
- Guards:
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `placed_carrier_present`
  - `cleanup_absent`
  - `runtime_live`
  - `fence_token_matches`
  - `generation_matches`
  - `member_retiring`
  - `remote_retirement_session_absent`
  - `session_binding_matches_or_cleared`
  - `kickoff_quiesced`
- Emits: `PlacedCarrierCleanupRequested`, `AppendLifecycleJournal`, `WiringGraphChanged`, `EmitMemberLifecycleNotice`
- To: `Running`

### `ObserveMemberRetirementArchivedPlacedLiveStopped`
- From: `Stopped`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal, preserve_machine_topology)
- Guards:
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `placed_carrier_present`
  - `cleanup_absent`
  - `runtime_live`
  - `fence_token_matches`
  - `generation_matches`
  - `member_retiring`
  - `remote_retirement_session_absent`
  - `session_binding_matches_or_cleared`
  - `kickoff_quiesced`
- Emits: `PlacedCarrierCleanupRequested`, `AppendLifecycleJournal`, `WiringGraphChanged`, `EmitMemberLifecycleNotice`
- To: `Stopped`

### `ObserveMemberRetirementArchivedPlacedRetiredRunning`
- From: `Running`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal, preserve_machine_topology)
- Guards:
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `placed_carrier_present`
  - `cleanup_absent`
  - `runtime_not_live`
  - `member_retiring`
  - `generation_matches`
  - `fence_token_matches_history`
  - `remote_retirement_session_absent`
  - `session_binding_matches_or_cleared`
  - `kickoff_quiesced`
- Emits: `PlacedCarrierCleanupRequested`, `AppendLifecycleJournal`, `WiringGraphChanged`
- To: `Running`

### `ObserveMemberRetirementArchivedPlacedRetiredStopped`
- From: `Stopped`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal, preserve_machine_topology)
- Guards:
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `placed_carrier_present`
  - `cleanup_absent`
  - `runtime_not_live`
  - `member_retiring`
  - `generation_matches`
  - `fence_token_matches_history`
  - `remote_retirement_session_absent`
  - `session_binding_matches_or_cleared`
  - `kickoff_quiesced`
- Emits: `PlacedCarrierCleanupRequested`, `AppendLifecycleJournal`, `WiringGraphChanged`
- To: `Stopped`

### `ObserveMemberRetirementArchivedStaleRuntime`
- From: `Running`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal)
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
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal)
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
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal)
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
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal)
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
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal)
- Guards:
  - `session_ingress_detach_closed`
  - `identity_remapped`
  - `runtime_not_live`
  - `member_not_retiring`
- Emits: `AppendLifecycleJournal`
- To: `Running`

### `ObserveMemberRetirementArchivedStaleRuntimeAlreadyClearedStopped`
- From: `Stopped`
- On: `ObserveMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal)
- Guards:
  - `session_ingress_detach_closed`
  - `identity_remapped`
  - `runtime_not_live`
  - `member_not_retiring`
- Emits: `AppendLifecycleJournal`
- To: `Stopped`

### `ObserveDestroyMemberRetirementArchivedLiveRunning`
- From: `Running`
- On: `ObserveDestroyMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal)
- Guards:
  - `session_ingress_detach_closed`
  - `destroy_admitted`
  - `identity_binding_matches`
  - `placed_carrier_absent`
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
- On: `ObserveDestroyMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal)
- Guards:
  - `session_ingress_detach_closed`
  - `destroy_admitted`
  - `identity_binding_matches`
  - `placed_carrier_absent`
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
- On: `ObserveDestroyMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal)
- Guards:
  - `session_ingress_detach_closed`
  - `destroy_admitted`
  - `identity_binding_matches`
  - `placed_carrier_absent`
  - `generation_matches`
  - `fence_token_matches_history`
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
- On: `ObserveDestroyMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal)
- Guards:
  - `session_ingress_detach_closed`
  - `destroy_admitted`
  - `identity_binding_matches`
  - `placed_carrier_absent`
  - `generation_matches`
  - `fence_token_matches_history`
  - `session_binding_matches`
  - `session_present`
  - `runtime_not_live`
  - `member_retiring`
  - `retirement_session_matches`
  - `kickoff_quiesced`
- Emits: `AppendLifecycleJournal`, `MemberSessionBindingChanged`
- To: `Stopped`

### `ObserveDestroyMemberRetirementArchivedPlacedLiveRunning`
- From: `Running`
- On: `ObserveDestroyMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal)
- Guards:
  - `destroy_admitted`
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `placed_carrier_present`
  - `cleanup_absent`
  - `generation_matches`
  - `session_present`
  - `session_binding_matches`
  - `remote_retirement_session_absent`
  - `runtime_live`
  - `fence_token_matches`
  - `member_retiring`
  - `kickoff_quiesced`
- Emits: `PlacedCarrierCleanupRequested`, `AppendLifecycleJournal`, `EmitMemberLifecycleNotice`, `MemberSessionBindingChanged`
- To: `Running`

### `ObserveDestroyMemberRetirementArchivedPlacedLiveStopped`
- From: `Stopped`
- On: `ObserveDestroyMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal)
- Guards:
  - `destroy_admitted`
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `placed_carrier_present`
  - `cleanup_absent`
  - `generation_matches`
  - `session_present`
  - `session_binding_matches`
  - `remote_retirement_session_absent`
  - `runtime_live`
  - `fence_token_matches`
  - `member_retiring`
  - `kickoff_quiesced`
- Emits: `PlacedCarrierCleanupRequested`, `AppendLifecycleJournal`, `EmitMemberLifecycleNotice`, `MemberSessionBindingChanged`
- To: `Stopped`

### `ObserveDestroyMemberRetirementArchivedPlacedRetiredRunning`
- From: `Running`
- On: `ObserveDestroyMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal)
- Guards:
  - `destroy_admitted`
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `placed_carrier_present`
  - `cleanup_absent`
  - `generation_matches`
  - `fence_token_matches_history`
  - `session_present`
  - `session_binding_matches`
  - `remote_retirement_session_absent`
  - `runtime_not_live`
  - `member_retiring`
  - `kickoff_quiesced`
- Emits: `PlacedCarrierCleanupRequested`, `AppendLifecycleJournal`, `MemberSessionBindingChanged`
- To: `Running`

### `ObserveDestroyMemberRetirementArchivedPlacedRetiredStopped`
- From: `Stopped`
- On: `ObserveDestroyMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal)
- Guards:
  - `destroy_admitted`
  - `session_ingress_detach_closed`
  - `identity_binding_matches`
  - `placed_carrier_present`
  - `cleanup_absent`
  - `generation_matches`
  - `fence_token_matches_history`
  - `session_present`
  - `session_binding_matches`
  - `remote_retirement_session_absent`
  - `runtime_not_live`
  - `member_retiring`
  - `kickoff_quiesced`
- Emits: `PlacedCarrierCleanupRequested`, `AppendLifecycleJournal`, `MemberSessionBindingChanged`
- To: `Stopped`

### `ObserveDestroyMemberRetirementArchivedAlreadyClearedRunning`
- From: `Running`
- On: `ObserveDestroyMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal)
- Guards:
  - `destroy_admitted`
  - `session_ingress_detach_closed`
  - `identity_binding_cleared`
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
- On: `ObserveDestroyMemberRetirementArchived`(agent_identity, agent_runtime_id, fence_token, generation, session_id, disposal)
- Guards:
  - `destroy_admitted`
  - `session_ingress_detach_closed`
  - `identity_binding_cleared`
  - `generation_matches`
  - `fence_token_matches`
  - `runtime_not_live`
  - `member_not_retiring`
  - `session_binding_cleared`
  - `retry_observes_cleared_session`
- Emits: `AppendLifecycleJournal`
- To: `Stopped`

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
  - `lifecycle_origin_open`
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
  - `placed_carrier_binding_active_or_local`
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
- Guards:
  - `adaptive_lifecycle_drained`
  - `placed_completion_quiesce_started`
  - `placed_completion_destroy_intent`
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
  - `placed_completion_quiesce_started`
  - `placed_completion_complete_intent`
  - `placed_completion_pending_drained`
  - `placed_completion_cancel_requested_drained`
  - `placed_completion_resolved_drained`
- Emits: `EmitMemberLifecycleNotice`
- To: `Completed`

### `DestroyMob`
- From: `Running`, `Stopped`, `Completed`
- On: `DestroyMob`(session_id)
- Guards:
  - `session_ingress_detaches_closed`
  - `placed_completion_quiesce_started`
  - `placed_completion_destroy_intent`
  - `remote_turn_pending_drained`
  - `remote_turn_committed_drained`
  - `remote_turn_resolved_drained`
  - `placed_completion_pending_drained`
  - `placed_completion_cancel_requested_drained`
  - `placed_completion_resolved_drained`
  - `placed_kickoff_pending_drained`
  - `placed_kickoff_resolved_drained`
  - `host_bind_phases_drained`
  - `replacement_host_bind_requests_drained`
  - `bound_hosts_drained`
  - `host_authority_epochs_drained`
  - `host_public_keys_drained`
  - `host_endpoints_drained`
  - `host_live_endpoints_drained`
  - `placed_carrier_cleanup_drained`
  - `pending_placed_spawns_drained`
  - `committed_placed_spawns_drained`
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
  - `lifecycle_origin_open`
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
  - `adaptive_lifecycle_drained`
  - `no_active_runs`
  - `placed_completion_quiesce_started`
  - `placed_completion_stop_intent`
  - `placed_completion_pending_drained`
  - `placed_completion_cancel_requested_drained`
  - `placed_completion_resolved_drained`
- Emits: `AppendLifecycleJournal`, `EmitRunLifecycleNotice`
- To: `Stopped`

### `ResumeStopped`
- From: `Stopped`
- On: `Resume`()
- Guards:
  - `placed_completion_stop_intent`
- Emits: `PersistPlacedCompletionLifecycleIntent`, `AppendLifecycleJournal`, `EmitRunLifecycleNotice`
- To: `Running`

### `CompleteRunning`
- From: `Running`
- On: `Complete`()
- Guards:
  - `adaptive_lifecycle_drained`
  - `placed_completion_quiesce_started`
  - `placed_completion_complete_intent`
  - `placed_completion_pending_drained`
  - `placed_completion_cancel_requested_drained`
  - `placed_completion_resolved_drained`
- Emits: `AppendLifecycleJournal`, `EmitRunLifecycleNotice`
- To: `Completed`

### `ResetToRunning`
- From: `Running`, `Stopped`, `Completed`
- On: `Reset`()
- Guards:
  - `adaptive_lifecycle_drained`
  - `remote_turn_pending_drained`
  - `remote_turn_committed_drained`
  - `remote_turn_resolved_drained`
  - `placed_completion_pending_drained`
  - `placed_completion_cancel_requested_drained`
  - `placed_completion_resolved_drained`
  - `placed_completion_reset_intent`
  - `placed_kickoff_pending_drained`
  - `placed_kickoff_resolved_drained`
- Emits: `AppendLifecycleJournal`, `EmitRunLifecycleNotice`, `WiringGraphChanged`
- To: `Running`

### `WireMembersRunning`
- From: `Running`
- On: `WireMembers`(edge)
- Guards:
  - `lifecycle_origin_open`
  - `edge_not_already_wired`
- Emits: `WiringGraphChanged`, `EmitWiringLifecycleNotice`
- To: `Running`

### `WireMembersWithTrustRunning`
- From: `Running`
- On: `WireMembersWithTrust`(edge, a_identity, b_identity)
- Guards:
  - `lifecycle_origin_open`
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

### `ConvergeRecoveredRosterTopologyPrune`
- From: `Running`
- On: `ConvergeRecoveredRosterTopology`(edge, a_identity, b_identity)
- Guards:
  - `edge_recovered`
  - `edge_matches_members`
  - `edge_has_absent_endpoint`
- To: `Running`

### `ConvergeRecoveredRosterTopologyRetain`
- From: `Running`
- On: `ConvergeRecoveredRosterTopology`(edge, a_identity, b_identity)
- Guards:
  - `edge_recovered`
  - `edge_matches_members`
  - `edge_endpoints_present`
- To: `Running`

### `ConvergeRecoveredRosterTopologyDestroyed`
- From: `Destroyed`
- On: `ConvergeRecoveredRosterTopology`(edge, a_identity, b_identity)
- To: `Destroyed`

### `ConvergeRecoveredRespawnTopologyAbandoned`
- From: `Running`
- On: `ConvergeRecoveredRespawnTopology`(agent_identity)
- Guards:
  - `respawn_topology_pending`
  - `successor_absent`
- To: `Running`

### `ConvergeRecoveredRespawnTopologyDestroyed`
- From: `Destroyed`
- On: `ConvergeRecoveredRespawnTopology`(agent_identity)
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
  - `lifecycle_origin_open`
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
- On: `RegisterMemberPeer`(agent_identity, agent_runtime_id, generation, fence_token, peer_endpoint)
- Guards:
  - `identity_present`
  - `identity_runtime_matches`
  - `runtime_live`
  - `generation_matches`
  - `identity_fence_matches`
  - `runtime_fence_matches`
  - `member_not_retiring`
  - `member_peer_not_registered`
  - `member_endpoint_not_registered`
  - `fresh_spawn_or_no_retained_topology`
  - `peer_id_available_for_identity`
  - `member_peer_id_is_not_host`
- Emits: `MemberPeerRegistered`
- To: `Running`

### `RegisterMemberPeerAlreadyCurrentRunning`
- From: `Running`
- On: `RegisterMemberPeer`(agent_identity, agent_runtime_id, generation, fence_token, peer_endpoint)
- Guards:
  - `identity_present`
  - `identity_runtime_matches`
  - `runtime_live`
  - `generation_matches`
  - `identity_fence_matches`
  - `runtime_fence_matches`
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

### `AuthorizeRetiringMemberPeerOverlayCleanupRunning`
- From: `Running`
- On: `AuthorizeRetiringMemberPeerOverlayCleanup`(recipient_identity, expected_recipient_endpoint, retiring_identity, retiring_runtime_id, fence_token, generation)
- Guards:
  - `recipient_present`
  - `recipient_endpoint_registered`
  - `recipient_endpoint_matches`
  - `recipient_peer_id_matches`
  - `retiring_binding_matches`
  - `retiring_generation_matches`
  - `retiring_fence_matches`
  - `retiring_member_marked`
  - `respawn_topology_preservation_recorded`
  - `recipient_wired_to_retiring`
  - `filtered_overlay_member_endpoints_complete`
  - `filtered_overlay_peer_ids_unique`
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
  - `lifecycle_origin_open`
  - `destroy_not_admitted`
  - `no_member_retiring`
- To: `Running`

### `AdmitSupervisorRotationStopped`
- From: `Stopped`
- On: `AdmitSupervisorRotation`()
- Guards:
  - `lifecycle_origin_open`
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

### `BeginHostBindFreshUnplaced`
- From: `Running`
- On: `BeginHostBind`(host_id, expected_endpoint, binding_generation)
- Guards:
  - `lifecycle_origin_open`
  - `host_untracked`
  - `no_replacement_request`
  - `no_retained_placement`
  - `binding_generation_positive`
  - `binding_generation_advances`
- Emits: `RequestHostBind`
- To: `Running`

### `BeginHostBindFreshReplacement`
- From: `Running`
- On: `BeginHostBind`(host_id, expected_endpoint, binding_generation)
- Guards:
  - `lifecycle_origin_open`
  - `host_untracked`
  - `no_replacement_request`
  - `retained_placement_present`
  - `binding_generation_positive`
  - `binding_generation_advances`
- Emits: `RequestHostBind`
- To: `Running`

### `BeginHostBindRequested`
- From: `Running`
- On: `BeginHostBind`(host_id, expected_endpoint, binding_generation)
- Guards:
  - `lifecycle_origin_open`
  - `host_requested`
  - `binding_generation_exact`
- Emits: `RequestHostBind`
- To: `Running`

### `BeginHostBindReplacementRequested`
- From: `Running`
- On: `BeginHostBind`(host_id, expected_endpoint, binding_generation)
- Guards:
  - `lifecycle_origin_open`
  - `host_phase_absent`
  - `replacement_endpoint_exact`
  - `binding_generation_exact`
- Emits: `RequestHostBind`
- To: `Running`

### `CommitHostBind`
- From: `Running`
- On: `CommitHostBind`(host_id, pubkey, endpoint, epoch, binding_generation, protocol_min, protocol_max, engine_version, durable_sessions, autonomous_members, hard_cancel_member, tracked_input_cancel, memory_store, mcp, resolvable_providers, approval_forwarding, live_endpoint)
- Guards:
  - `bind_requested`
  - `binding_generation_exact`
  - `replacement_endpoint_exact`
  - `active_capability_contract_preserved`
- Emits: `HostRegistered`
- To: `Running`

### `HostRebound`
- From: `Running`
- On: `HostRebound`(host_id, epoch, binding_generation, protocol_min, protocol_max, engine_version, durable_sessions, autonomous_members, hard_cancel_member, tracked_input_cancel, memory_store, mcp, resolvable_providers, approval_forwarding, live_endpoint)
- Guards:
  - `host_bound`
  - `binding_generation_exact`
  - `epoch_advances`
  - `active_capability_contract_preserved`
- Emits: `HostReboundRecorded`
- To: `Running`

### `RefreshHostCapabilities`
- From: `Running`
- On: `RefreshHostCapabilities`(host_id, epoch, binding_generation, protocol_min, protocol_max, engine_version, durable_sessions, autonomous_members, hard_cancel_member, tracked_input_cancel, memory_store, mcp, resolvable_providers, approval_forwarding, live_endpoint)
- Guards:
  - `host_bound`
  - `host_epoch_exact`
  - `binding_generation_exact`
  - `active_capability_contract_preserved`
- Emits: `HostCapabilitiesRefreshed`
- To: `Running`

### `RevokeHost`
- From: `Running`
- On: `RevokeHost`(host_id, binding_generation)
- Guards:
  - `host_tracked`
  - `binding_generation_exact`
  - `placed_kickoff_custody_drained_for_host`
- Emits: `HostRevoked`
- To: `Running`

### `RecoverOrdinaryHostBindRequestRunning`
- From: `Running`
- On: `RecoverHostBindRequest`(host_id, expected_endpoint, binding_generation, replacement)
- Guards:
  - `ordinary_request`
  - `host_untracked`
  - `host_generation_absent`
  - `replacement_absent`
  - `binding_generation_positive`
  - `binding_generation_not_regressed`
- To: `Running`

### `RecoverOrdinaryHostBindRequestStopped`
- From: `Stopped`
- On: `RecoverHostBindRequest`(host_id, expected_endpoint, binding_generation, replacement)
- Guards:
  - `ordinary_request`
  - `host_untracked`
  - `host_generation_absent`
  - `replacement_absent`
  - `binding_generation_positive`
  - `binding_generation_not_regressed`
- To: `Stopped`

### `RecoverOrdinaryHostBindRequestCompleted`
- From: `Completed`
- On: `RecoverHostBindRequest`(host_id, expected_endpoint, binding_generation, replacement)
- Guards:
  - `ordinary_request`
  - `host_untracked`
  - `host_generation_absent`
  - `replacement_absent`
  - `binding_generation_positive`
  - `binding_generation_not_regressed`
- To: `Completed`

### `RecoverOrdinaryHostBindRequestDestroyed`
- From: `Destroyed`
- On: `RecoverHostBindRequest`(host_id, expected_endpoint, binding_generation, replacement)
- Guards:
  - `ordinary_request`
  - `host_untracked`
  - `host_generation_absent`
  - `replacement_absent`
  - `binding_generation_positive`
  - `binding_generation_not_regressed`
- To: `Destroyed`

### `RecoverReplacementHostBindRequestRunning`
- From: `Running`
- On: `RecoverHostBindRequest`(host_id, expected_endpoint, binding_generation, replacement)
- Guards:
  - `replacement_request`
  - `host_untracked`
  - `host_generation_absent`
  - `replacement_absent`
  - `binding_generation_positive`
  - `binding_generation_not_regressed`
- To: `Running`

### `RecoverReplacementHostBindRequestStopped`
- From: `Stopped`
- On: `RecoverHostBindRequest`(host_id, expected_endpoint, binding_generation, replacement)
- Guards:
  - `replacement_request`
  - `host_untracked`
  - `host_generation_absent`
  - `replacement_absent`
  - `binding_generation_positive`
  - `binding_generation_not_regressed`
- To: `Stopped`

### `RecoverReplacementHostBindRequestCompleted`
- From: `Completed`
- On: `RecoverHostBindRequest`(host_id, expected_endpoint, binding_generation, replacement)
- Guards:
  - `replacement_request`
  - `host_untracked`
  - `host_generation_absent`
  - `replacement_absent`
  - `binding_generation_positive`
  - `binding_generation_not_regressed`
- To: `Completed`

### `RecoverReplacementHostBindRequestDestroyed`
- From: `Destroyed`
- On: `RecoverHostBindRequest`(host_id, expected_endpoint, binding_generation, replacement)
- Guards:
  - `replacement_request`
  - `host_untracked`
  - `host_generation_absent`
  - `replacement_absent`
  - `binding_generation_positive`
  - `binding_generation_not_regressed`
- To: `Destroyed`

### `RecoverHostBindingRunning`
- From: `Running`
- On: `RecoverHostBinding`(host_id, pubkey, endpoint, epoch, binding_generation, protocol_min, protocol_max, engine_version, durable_sessions, autonomous_members, hard_cancel_member, tracked_input_cancel, memory_store, mcp, resolvable_providers, approval_forwarding, live_endpoint)
- Guards:
  - `host_untracked`
  - `binding_generation_not_regressed`
- To: `Running`

### `RecoverHostBindingStopped`
- From: `Stopped`
- On: `RecoverHostBinding`(host_id, pubkey, endpoint, epoch, binding_generation, protocol_min, protocol_max, engine_version, durable_sessions, autonomous_members, hard_cancel_member, tracked_input_cancel, memory_store, mcp, resolvable_providers, approval_forwarding, live_endpoint)
- Guards:
  - `host_untracked`
  - `binding_generation_not_regressed`
- To: `Stopped`

### `RecoverHostBindingCompleted`
- From: `Completed`
- On: `RecoverHostBinding`(host_id, pubkey, endpoint, epoch, binding_generation, protocol_min, protocol_max, engine_version, durable_sessions, autonomous_members, hard_cancel_member, tracked_input_cancel, memory_store, mcp, resolvable_providers, approval_forwarding, live_endpoint)
- Guards:
  - `host_untracked`
  - `binding_generation_not_regressed`
- To: `Completed`

### `RecoverHostBindingDestroyed`
- From: `Destroyed`
- On: `RecoverHostBinding`(host_id, pubkey, endpoint, epoch, binding_generation, protocol_min, protocol_max, engine_version, durable_sessions, autonomous_members, hard_cancel_member, tracked_input_cancel, memory_store, mcp, resolvable_providers, approval_forwarding, live_endpoint)
- Guards:
  - `host_untracked`
  - `binding_generation_not_regressed`
- To: `Destroyed`

### `RecoverHostBindingGenerationHighwaterRunning`
- From: `Running`
- On: `RecoverHostBindingGenerationHighwater`(host_id, binding_generation)
- Guards:
  - `binding_generation_positive`
  - `binding_generation_not_regressed`
- To: `Running`

### `RecoverHostBindingGenerationHighwaterStopped`
- From: `Stopped`
- On: `RecoverHostBindingGenerationHighwater`(host_id, binding_generation)
- Guards:
  - `binding_generation_positive`
  - `binding_generation_not_regressed`
- To: `Stopped`

### `RecoverHostBindingGenerationHighwaterCompleted`
- From: `Completed`
- On: `RecoverHostBindingGenerationHighwater`(host_id, binding_generation)
- Guards:
  - `binding_generation_positive`
  - `binding_generation_not_regressed`
- To: `Completed`

### `RecoverHostBindingGenerationHighwaterDestroyed`
- From: `Destroyed`
- On: `RecoverHostBindingGenerationHighwater`(host_id, binding_generation)
- Guards:
  - `binding_generation_positive`
  - `binding_generation_not_regressed`
- To: `Destroyed`

### `RecoverConfirmedHostBindingRevocationRunning`
- From: `Running`
- On: `RecoverConfirmedHostBindingRevocation`(host_id, binding_generation)
- Guards:
  - `binding_generation_positive`
- To: `Running`

### `RecoverConfirmedHostBindingRevocationStopped`
- From: `Stopped`
- On: `RecoverConfirmedHostBindingRevocation`(host_id, binding_generation)
- Guards:
  - `binding_generation_positive`
- To: `Stopped`

### `RecoverConfirmedHostBindingRevocationCompleted`
- From: `Completed`
- On: `RecoverConfirmedHostBindingRevocation`(host_id, binding_generation)
- Guards:
  - `binding_generation_positive`
- To: `Completed`

### `RecoverConfirmedHostBindingRevocationDestroyed`
- From: `Destroyed`
- On: `RecoverConfirmedHostBindingRevocation`(host_id, binding_generation)
- Guards:
  - `binding_generation_positive`
- To: `Destroyed`

### `RecordRouteInstallInstall`
- From: `Running`
- On: `RecordRouteInstall`(obligation)
- Guards:
  - `lifecycle_origin_open`
  - `obligation_is_install`
  - `edge_currently_wired`
  - `host_bound`
  - `placed_endpoints_binding_active`
- Emits: `RouteInstallRequested`
- To: `Running`

### `AuthorizeRouteRemovalBeforeUnwire`
- From: `Running`
- On: `AuthorizeRouteRemovalBeforeUnwire`(obligation)
- Guards:
  - `obligation_is_remove`
  - `edge_currently_wired`
  - `host_bound`
  - `placed_endpoints_binding_active`
  - `host_owns_edge_endpoint`
  - `edge_endpoints_published`
- Emits: `RouteInstallRequested`
- To: `Running`

### `ResolveRouteInstallRunning`
- From: `Running`
- On: `ResolveRouteInstall`(obligation)
- Guards:
  - `obligation_is_install`
- To: `Running`

### `ResolveRouteInstallStopped`
- From: `Stopped`
- On: `ResolveRouteInstall`(obligation)
- Guards:
  - `obligation_is_install`
- To: `Stopped`

### `ResolveRouteInstallCompleted`
- From: `Completed`
- On: `ResolveRouteInstall`(obligation)
- Guards:
  - `obligation_is_install`
- To: `Completed`

### `ResolveRouteInstallDestroyed`
- From: `Destroyed`
- On: `ResolveRouteInstall`(obligation)
- Guards:
  - `obligation_is_install`
- To: `Destroyed`

### `RollbackRouteInstallRunning`
- From: `Running`
- On: `RollbackRouteInstall`(obligation)
- Guards:
  - `obligation_is_install`
- To: `Running`

### `RollbackRouteInstallStopped`
- From: `Stopped`
- On: `RollbackRouteInstall`(obligation)
- Guards:
  - `obligation_is_install`
- To: `Stopped`

### `RollbackRouteInstallCompleted`
- From: `Completed`
- On: `RollbackRouteInstall`(obligation)
- Guards:
  - `obligation_is_install`
- To: `Completed`

### `RollbackRouteInstallDestroyed`
- From: `Destroyed`
- On: `RollbackRouteInstall`(obligation)
- Guards:
  - `obligation_is_install`
- To: `Destroyed`

### `RecoverRemoteTurnDispatchSequenceAdvance`
- From: `Running`
- On: `RecoverRemoteTurnDispatchSequence`(dispatch_sequence)
- Guards:
  - `newer_dispatch_sequence`
- To: `Running`

### `RecoverRemoteTurnDispatchSequenceReplay`
- From: `Running`
- On: `RecoverRemoteTurnDispatchSequence`(dispatch_sequence)
- Guards:
  - `historical_dispatch_sequence`
- To: `Running`

### `RecordRemoteTurnObligationFresh`
- From: `Running`
- On: `RecordRemoteTurnObligation`(obligation)
- Guards:
  - `lifecycle_origin_open`
  - `target_placed`
  - `target_host_exact`
  - `target_host_bound`
  - `target_carrier_binding_active`
  - `target_host_binding_generation_exact`
  - `target_host_durable_sessions`
  - `target_host_supports_tracked_input_cancel`
  - `target_host_supports_v4`
  - `target_session_exact`
  - `target_runtime_present`
  - `target_runtime_live`
  - `target_not_retiring`
  - `target_not_reviving`
  - `target_spawn_settled`
  - `target_materialization_healthy`
  - `custody_quota_and_correlation_available`
  - `placed_kickoff_correlation_available`
  - `placed_completion_correlation_available`
  - `current_generation`
  - `current_fence`
  - `newer_dispatch_sequence`
  - `not_pending`
  - `not_committed`
  - `not_resolved`
- To: `Running`

### `RecordRemoteTurnObligationPendingReplay`
- From: `Running`
- On: `RecordRemoteTurnObligation`(obligation)
- Guards:
  - `already_pending`
- To: `Running`

### `RecordRemoteTurnObligationCommittedReplay`
- From: `Running`
- On: `RecordRemoteTurnObligation`(obligation)
- Guards:
  - `already_committed`
- To: `Running`

### `RecordRemoteTurnObligationResolvedReplay`
- From: `Running`
- On: `RecordRemoteTurnObligation`(obligation)
- Guards:
  - `already_resolved`
- To: `Running`

### `RecordRemoteTurnObligationHistoricalReplay`
- From: `Running`
- On: `RecordRemoteTurnObligation`(obligation)
- Guards:
  - `historical_sequence`
  - `placed_kickoff_correlation_available`
  - `not_pending`
  - `not_committed`
  - `not_resolved`
- To: `Running`

### `AbortRemoteTurnObligationPresentRunning`
- From: `Running`
- On: `AbortRemoteTurnObligation`(obligation)
- Guards:
  - `pending`
- To: `Running`

### `AbortRemoteTurnObligationPresentStopped`
- From: `Stopped`
- On: `AbortRemoteTurnObligation`(obligation)
- Guards:
  - `pending`
- To: `Stopped`

### `AbortRemoteTurnObligationPresentCompleted`
- From: `Completed`
- On: `AbortRemoteTurnObligation`(obligation)
- Guards:
  - `pending`
- To: `Completed`

### `AbortRemoteTurnObligationPresentDestroyed`
- From: `Destroyed`
- On: `AbortRemoteTurnObligation`(obligation)
- Guards:
  - `pending`
- To: `Destroyed`

### `AbortRemoteTurnObligationReplayRunning`
- From: `Running`
- On: `AbortRemoteTurnObligation`(obligation)
- Guards:
  - `already_absent`
- To: `Running`

### `AbortRemoteTurnObligationReplayStopped`
- From: `Stopped`
- On: `AbortRemoteTurnObligation`(obligation)
- Guards:
  - `already_absent`
- To: `Stopped`

### `AbortRemoteTurnObligationReplayCompleted`
- From: `Completed`
- On: `AbortRemoteTurnObligation`(obligation)
- Guards:
  - `already_absent`
- To: `Completed`

### `AbortRemoteTurnObligationReplayDestroyed`
- From: `Destroyed`
- On: `AbortRemoteTurnObligation`(obligation)
- Guards:
  - `already_absent`
- To: `Destroyed`

### `CommitRemoteTurnOutcomePendingRunning`
- From: `Running`
- On: `CommitRemoteTurnOutcome`(obligation)
- Guards:
  - `pending`
- To: `Running`

### `CommitRemoteTurnOutcomePendingStopped`
- From: `Stopped`
- On: `CommitRemoteTurnOutcome`(obligation)
- Guards:
  - `pending`
- To: `Stopped`

### `CommitRemoteTurnOutcomePendingCompleted`
- From: `Completed`
- On: `CommitRemoteTurnOutcome`(obligation)
- Guards:
  - `pending`
- To: `Completed`

### `CommitRemoteTurnOutcomePendingDestroyed`
- From: `Destroyed`
- On: `CommitRemoteTurnOutcome`(obligation)
- Guards:
  - `pending`
- To: `Destroyed`

### `CommitRemoteTurnOutcomeReplayRunning`
- From: `Running`
- On: `CommitRemoteTurnOutcome`(obligation)
- Guards:
  - `already_committed_or_resolved`
- To: `Running`

### `CommitRemoteTurnOutcomeReplayStopped`
- From: `Stopped`
- On: `CommitRemoteTurnOutcome`(obligation)
- Guards:
  - `already_committed_or_resolved`
- To: `Stopped`

### `CommitRemoteTurnOutcomeReplayCompleted`
- From: `Completed`
- On: `CommitRemoteTurnOutcome`(obligation)
- Guards:
  - `already_committed_or_resolved`
- To: `Completed`

### `CommitRemoteTurnOutcomeReplayDestroyed`
- From: `Destroyed`
- On: `CommitRemoteTurnOutcome`(obligation)
- Guards:
  - `already_committed_or_resolved`
- To: `Destroyed`

### `CommitRemoteTurnOutcomeDisposedReplayRunning`
- From: `Running`
- On: `CommitRemoteTurnOutcome`(obligation)
- Guards:
  - `historical_sequence`
  - `already_absent`
- To: `Running`

### `CommitRemoteTurnOutcomeDisposedReplayStopped`
- From: `Stopped`
- On: `CommitRemoteTurnOutcome`(obligation)
- Guards:
  - `historical_sequence`
  - `already_absent`
- To: `Stopped`

### `CommitRemoteTurnOutcomeDisposedReplayCompleted`
- From: `Completed`
- On: `CommitRemoteTurnOutcome`(obligation)
- Guards:
  - `historical_sequence`
  - `already_absent`
- To: `Completed`

### `CommitRemoteTurnOutcomeDisposedReplayDestroyed`
- From: `Destroyed`
- On: `CommitRemoteTurnOutcome`(obligation)
- Guards:
  - `historical_sequence`
  - `already_absent`
- To: `Destroyed`

### `ResolveRemoteTurnObligationCommittedRunning`
- From: `Running`
- On: `ResolveRemoteTurnObligation`(obligation)
- Guards:
  - `committed`
- To: `Running`

### `ResolveRemoteTurnObligationCommittedStopped`
- From: `Stopped`
- On: `ResolveRemoteTurnObligation`(obligation)
- Guards:
  - `committed`
- To: `Stopped`

### `ResolveRemoteTurnObligationCommittedCompleted`
- From: `Completed`
- On: `ResolveRemoteTurnObligation`(obligation)
- Guards:
  - `committed`
- To: `Completed`

### `ResolveRemoteTurnObligationCommittedDestroyed`
- From: `Destroyed`
- On: `ResolveRemoteTurnObligation`(obligation)
- Guards:
  - `committed`
- To: `Destroyed`

### `ResolveRemoteTurnObligationReplayRunning`
- From: `Running`
- On: `ResolveRemoteTurnObligation`(obligation)
- Guards:
  - `already_resolved`
- To: `Running`

### `ResolveRemoteTurnObligationReplayStopped`
- From: `Stopped`
- On: `ResolveRemoteTurnObligation`(obligation)
- Guards:
  - `already_resolved`
- To: `Stopped`

### `ResolveRemoteTurnObligationReplayCompleted`
- From: `Completed`
- On: `ResolveRemoteTurnObligation`(obligation)
- Guards:
  - `already_resolved`
- To: `Completed`

### `ResolveRemoteTurnObligationReplayDestroyed`
- From: `Destroyed`
- On: `ResolveRemoteTurnObligation`(obligation)
- Guards:
  - `already_resolved`
- To: `Destroyed`

### `ResolveRemoteTurnObligationDisposedReplayRunning`
- From: `Running`
- On: `ResolveRemoteTurnObligation`(obligation)
- Guards:
  - `historical_sequence`
  - `already_absent`
- To: `Running`

### `ResolveRemoteTurnObligationDisposedReplayStopped`
- From: `Stopped`
- On: `ResolveRemoteTurnObligation`(obligation)
- Guards:
  - `historical_sequence`
  - `already_absent`
- To: `Stopped`

### `ResolveRemoteTurnObligationDisposedReplayCompleted`
- From: `Completed`
- On: `ResolveRemoteTurnObligation`(obligation)
- Guards:
  - `historical_sequence`
  - `already_absent`
- To: `Completed`

### `ResolveRemoteTurnObligationDisposedReplayDestroyed`
- From: `Destroyed`
- On: `ResolveRemoteTurnObligation`(obligation)
- Guards:
  - `historical_sequence`
  - `already_absent`
- To: `Destroyed`

### `AcknowledgeRemoteTurnOutcomePresentRunning`
- From: `Running`
- On: `AcknowledgeRemoteTurnOutcome`(obligation)
- Guards:
  - `resolved`
- To: `Running`

### `AcknowledgeRemoteTurnOutcomePresentStopped`
- From: `Stopped`
- On: `AcknowledgeRemoteTurnOutcome`(obligation)
- Guards:
  - `resolved`
- To: `Stopped`

### `AcknowledgeRemoteTurnOutcomePresentCompleted`
- From: `Completed`
- On: `AcknowledgeRemoteTurnOutcome`(obligation)
- Guards:
  - `resolved`
- To: `Completed`

### `AcknowledgeRemoteTurnOutcomePresentDestroyed`
- From: `Destroyed`
- On: `AcknowledgeRemoteTurnOutcome`(obligation)
- Guards:
  - `resolved`
- To: `Destroyed`

### `AcknowledgeRemoteTurnOutcomeReplayRunning`
- From: `Running`
- On: `AcknowledgeRemoteTurnOutcome`(obligation)
- Guards:
  - `already_absent`
- To: `Running`

### `AcknowledgeRemoteTurnOutcomeReplayStopped`
- From: `Stopped`
- On: `AcknowledgeRemoteTurnOutcome`(obligation)
- Guards:
  - `already_absent`
- To: `Stopped`

### `AcknowledgeRemoteTurnOutcomeReplayCompleted`
- From: `Completed`
- On: `AcknowledgeRemoteTurnOutcome`(obligation)
- Guards:
  - `already_absent`
- To: `Completed`

### `AcknowledgeRemoteTurnOutcomeReplayDestroyed`
- From: `Destroyed`
- On: `AcknowledgeRemoteTurnOutcome`(obligation)
- Guards:
  - `already_absent`
- To: `Destroyed`

### `DisposeRemoteTurnObligationRunning`
- From: `Running`
- On: `DisposeRemoteTurnObligation`(obligation)
- To: `Running`

### `DisposeRemoteTurnObligationStopped`
- From: `Stopped`
- On: `DisposeRemoteTurnObligation`(obligation)
- To: `Stopped`

### `DisposeRemoteTurnObligationCompleted`
- From: `Completed`
- On: `DisposeRemoteTurnObligation`(obligation)
- To: `Completed`

### `DisposeRemoteTurnObligationDestroyed`
- From: `Destroyed`
- On: `DisposeRemoteTurnObligation`(obligation)
- To: `Destroyed`

### `BeginPlacedCompletionLifecycleQuiesceFresh`
- From: `Running`
- On: `BeginPlacedCompletionLifecycleQuiesce`(intent)
- Guards:
  - `adaptive_lifecycle_drained`
  - `lifecycle_intent_admissible`
  - `not_quiescing`
- Emits: `PersistPlacedCompletionLifecycleIntent`
- To: `Running`

### `BeginPlacedCompletionLifecycleQuiesceReplay`
- From: `Running`
- On: `BeginPlacedCompletionLifecycleQuiesce`(intent)
- Guards:
  - `adaptive_lifecycle_drained`
  - `lifecycle_intent_admissible`
  - `already_quiescing`
  - `compatible_lifecycle_intent_takeover`
- Emits: `PersistPlacedCompletionLifecycleIntent`
- To: `Running`

### `BeginPlacedCompletionLifecycleQuiesceStoppedFresh`
- From: `Stopped`
- On: `BeginPlacedCompletionLifecycleQuiesce`(intent)
- Guards:
  - `adaptive_lifecycle_drained`
  - `lifecycle_intent_admissible`
  - `not_quiescing`
- Emits: `PersistPlacedCompletionLifecycleIntent`
- To: `Stopped`

### `BeginPlacedCompletionLifecycleQuiesceStoppedReplay`
- From: `Stopped`
- On: `BeginPlacedCompletionLifecycleQuiesce`(intent)
- Guards:
  - `adaptive_lifecycle_drained`
  - `lifecycle_intent_admissible`
  - `already_quiescing`
  - `compatible_lifecycle_intent_takeover`
- Emits: `PersistPlacedCompletionLifecycleIntent`
- To: `Stopped`

### `BeginPlacedCompletionLifecycleQuiesceCompletedFresh`
- From: `Completed`
- On: `BeginPlacedCompletionLifecycleQuiesce`(intent)
- Guards:
  - `adaptive_lifecycle_drained`
  - `lifecycle_intent_admissible`
  - `not_quiescing`
- Emits: `PersistPlacedCompletionLifecycleIntent`
- To: `Completed`

### `BeginPlacedCompletionLifecycleQuiesceCompletedReplay`
- From: `Completed`
- On: `BeginPlacedCompletionLifecycleQuiesce`(intent)
- Guards:
  - `adaptive_lifecycle_drained`
  - `lifecycle_intent_admissible`
  - `already_quiescing`
  - `compatible_lifecycle_intent_takeover`
- Emits: `PersistPlacedCompletionLifecycleIntent`
- To: `Completed`

### `EndPlacedCompletionLifecycleQuiesceRunningFresh`
- From: `Running`
- On: `EndPlacedCompletionLifecycleQuiesce`(intent)
- Guards:
  - `quiescing`
  - `same_lifecycle_intent`
  - `placed_completion_pending_drained`
  - `placed_completion_cancel_requested_drained`
  - `placed_completion_resolved_drained`
- Emits: `PersistPlacedCompletionLifecycleIntent`
- To: `Running`

### `EndPlacedCompletionLifecycleQuiesceRunningReplay`
- From: `Running`
- On: `EndPlacedCompletionLifecycleQuiesce`(intent)
- Guards:
  - `already_open`
  - `no_lifecycle_intent`
- Emits: `PersistPlacedCompletionLifecycleIntent`
- To: `Running`

### `EndPlacedCompletionLifecycleQuiesceStoppedRetireAll`
- From: `Stopped`
- On: `EndPlacedCompletionLifecycleQuiesce`(intent)
- Guards:
  - `retire_all_intent`
  - `retire_all_active`
  - `placed_completion_pending_drained`
  - `placed_completion_cancel_requested_drained`
  - `placed_completion_resolved_drained`
- Emits: `PersistPlacedCompletionLifecycleIntent`
- To: `Stopped`

### `EndPlacedCompletionLifecycleQuiesceStoppedRetireAllReplay`
- From: `Stopped`
- On: `EndPlacedCompletionLifecycleQuiesce`(intent)
- Guards:
  - `retire_all_intent`
  - `stop_baseline_restored`
- Emits: `PersistPlacedCompletionLifecycleIntent`
- To: `Stopped`

### `EndPlacedCompletionLifecycleQuiesceCompletedRetireAll`
- From: `Completed`
- On: `EndPlacedCompletionLifecycleQuiesce`(intent)
- Guards:
  - `retire_all_intent`
  - `retire_all_active`
  - `placed_completion_pending_drained`
  - `placed_completion_cancel_requested_drained`
  - `placed_completion_resolved_drained`
- Emits: `PersistPlacedCompletionLifecycleIntent`
- To: `Completed`

### `EndPlacedCompletionLifecycleQuiesceCompletedRetireAllReplay`
- From: `Completed`
- On: `EndPlacedCompletionLifecycleQuiesce`(intent)
- Guards:
  - `retire_all_intent`
  - `complete_baseline_restored`
- Emits: `PersistPlacedCompletionLifecycleIntent`
- To: `Completed`

### `RecoverPlacedCompletionDispatchSequenceAdvanceRunning`
- From: `Running`
- On: `RecoverPlacedCompletionDispatchSequence`(dispatch_sequence)
- Guards:
  - `newer_dispatch_sequence`
- To: `Running`

### `RecoverPlacedCompletionDispatchSequenceAdvanceStopped`
- From: `Stopped`
- On: `RecoverPlacedCompletionDispatchSequence`(dispatch_sequence)
- Guards:
  - `newer_dispatch_sequence`
- To: `Stopped`

### `RecoverPlacedCompletionDispatchSequenceAdvanceCompleted`
- From: `Completed`
- On: `RecoverPlacedCompletionDispatchSequence`(dispatch_sequence)
- Guards:
  - `newer_dispatch_sequence`
- To: `Completed`

### `RecoverPlacedCompletionDispatchSequenceAdvanceDestroyed`
- From: `Destroyed`
- On: `RecoverPlacedCompletionDispatchSequence`(dispatch_sequence)
- Guards:
  - `newer_dispatch_sequence`
- To: `Destroyed`

### `RecoverPlacedCompletionDispatchSequenceReplayRunning`
- From: `Running`
- On: `RecoverPlacedCompletionDispatchSequence`(dispatch_sequence)
- Guards:
  - `historical_dispatch_sequence`
- To: `Running`

### `RecoverPlacedCompletionDispatchSequenceReplayStopped`
- From: `Stopped`
- On: `RecoverPlacedCompletionDispatchSequence`(dispatch_sequence)
- Guards:
  - `historical_dispatch_sequence`
- To: `Stopped`

### `RecoverPlacedCompletionDispatchSequenceReplayCompleted`
- From: `Completed`
- On: `RecoverPlacedCompletionDispatchSequence`(dispatch_sequence)
- Guards:
  - `historical_dispatch_sequence`
- To: `Completed`

### `RecoverPlacedCompletionDispatchSequenceReplayDestroyed`
- From: `Destroyed`
- On: `RecoverPlacedCompletionDispatchSequence`(dispatch_sequence)
- Guards:
  - `historical_dispatch_sequence`
- To: `Destroyed`

### `RecoverPendingPlacedCompletionRunning`
- From: `Running`
- On: `RecoverPendingPlacedCompletion`(obligation, cancellation_requested)
- Guards:
  - `obligation_well_formed`
  - `target_host_exact`
  - `target_host_binding_generation_exact`
  - `target_session_exact`
  - `current_generation`
  - `current_fence`
  - `newer_dispatch_sequence`
  - `custody_available`
- To: `Running`

### `RecoverPendingPlacedCompletionStopped`
- From: `Stopped`
- On: `RecoverPendingPlacedCompletion`(obligation, cancellation_requested)
- Guards:
  - `obligation_well_formed`
  - `target_host_exact`
  - `target_host_binding_generation_exact`
  - `target_session_exact`
  - `current_generation`
  - `current_fence`
  - `newer_dispatch_sequence`
  - `custody_available`
- To: `Stopped`

### `RecoverPendingPlacedCompletionCompleted`
- From: `Completed`
- On: `RecoverPendingPlacedCompletion`(obligation, cancellation_requested)
- Guards:
  - `obligation_well_formed`
  - `target_host_exact`
  - `target_host_binding_generation_exact`
  - `target_session_exact`
  - `current_generation`
  - `current_fence`
  - `newer_dispatch_sequence`
  - `custody_available`
- To: `Completed`

### `RecoverResolvedPlacedCompletionRunning`
- From: `Running`
- On: `RecoverResolvedPlacedCompletion`(obligation)
- Guards:
  - `obligation_well_formed`
  - `target_host_exact`
  - `target_host_binding_generation_exact`
  - `target_session_exact`
  - `current_generation`
  - `current_fence`
  - `newer_dispatch_sequence`
  - `custody_available`
- To: `Running`

### `RecoverResolvedPlacedCompletionStopped`
- From: `Stopped`
- On: `RecoverResolvedPlacedCompletion`(obligation)
- Guards:
  - `obligation_well_formed`
  - `target_host_exact`
  - `target_host_binding_generation_exact`
  - `target_session_exact`
  - `current_generation`
  - `current_fence`
  - `newer_dispatch_sequence`
  - `custody_available`
- To: `Stopped`

### `RecoverResolvedPlacedCompletionCompleted`
- From: `Completed`
- On: `RecoverResolvedPlacedCompletion`(obligation)
- Guards:
  - `obligation_well_formed`
  - `target_host_exact`
  - `target_host_binding_generation_exact`
  - `target_session_exact`
  - `current_generation`
  - `current_fence`
  - `newer_dispatch_sequence`
  - `custody_available`
- To: `Completed`

### `RecoverCompletedWithCleanupCustody`
- From: `Running`
- On: `RecoverCompletedWithCleanupCustody`()
- Guards:
  - `placed_completion_quiesce_started`
  - `placed_completion_complete_intent`
  - `cleanup_custody_present`
- To: `Completed`

### `RecordPlacedCompletionObligationFresh`
- From: `Running`
- On: `RecordPlacedCompletionObligation`(obligation)
- Guards:
  - `placed_completion_origin_open`
  - `obligation_well_formed`
  - `target_host_exact`
  - `target_host_bound`
  - `target_carrier_binding_active`
  - `target_host_binding_generation_exact`
  - `target_host_supports_tracked_cancel`
  - `target_session_exact`
  - `target_runtime_present`
  - `target_runtime_live`
  - `target_not_retiring`
  - `target_not_reviving`
  - `target_spawn_settled`
  - `target_materialization_healthy`
  - `current_generation`
  - `current_fence`
  - `custody_quota_and_correlation_available`
  - `flow_correlation_available`
  - `kickoff_correlation_available`
  - `newer_dispatch_sequence`
- To: `Running`

### `RecordPlacedCompletionObligationPendingReplayRunning`
- From: `Running`
- On: `RecordPlacedCompletionObligation`(obligation)
- Guards:
  - `already_pending`
- To: `Running`

### `RecordPlacedCompletionObligationPendingReplayStopped`
- From: `Stopped`
- On: `RecordPlacedCompletionObligation`(obligation)
- Guards:
  - `already_pending`
- To: `Stopped`

### `RecordPlacedCompletionObligationPendingReplayCompleted`
- From: `Completed`
- On: `RecordPlacedCompletionObligation`(obligation)
- Guards:
  - `already_pending`
- To: `Completed`

### `RecordPlacedCompletionObligationResolvedReplayRunning`
- From: `Running`
- On: `RecordPlacedCompletionObligation`(obligation)
- Guards:
  - `already_resolved`
- To: `Running`

### `RecordPlacedCompletionObligationResolvedReplayStopped`
- From: `Stopped`
- On: `RecordPlacedCompletionObligation`(obligation)
- Guards:
  - `already_resolved`
- To: `Stopped`

### `RecordPlacedCompletionObligationResolvedReplayCompleted`
- From: `Completed`
- On: `RecordPlacedCompletionObligation`(obligation)
- Guards:
  - `already_resolved`
- To: `Completed`

### `RecordPlacedCompletionObligationHistoricalReplayRunning`
- From: `Running`
- On: `RecordPlacedCompletionObligation`(obligation)
- Guards:
  - `historical_sequence`
  - `custody_absent`
- To: `Running`

### `RecordPlacedCompletionObligationHistoricalReplayStopped`
- From: `Stopped`
- On: `RecordPlacedCompletionObligation`(obligation)
- Guards:
  - `historical_sequence`
  - `custody_absent`
- To: `Stopped`

### `RecordPlacedCompletionObligationHistoricalReplayCompleted`
- From: `Completed`
- On: `RecordPlacedCompletionObligation`(obligation)
- Guards:
  - `historical_sequence`
  - `custody_absent`
- To: `Completed`

### `RecordPlacedCompletionObligationHistoricalReplayDestroyed`
- From: `Destroyed`
- On: `RecordPlacedCompletionObligation`(obligation)
- Guards:
  - `historical_sequence`
  - `custody_absent`
- To: `Destroyed`

### `RequestPlacedCompletionCancellationPendingRunning`
- From: `Running`
- On: `RequestPlacedCompletionCancellation`(obligation)
- Guards:
  - `pending`
  - `not_requested`
- To: `Running`

### `RequestPlacedCompletionCancellationPendingStopped`
- From: `Stopped`
- On: `RequestPlacedCompletionCancellation`(obligation)
- Guards:
  - `pending`
  - `not_requested`
- To: `Stopped`

### `RequestPlacedCompletionCancellationPendingCompleted`
- From: `Completed`
- On: `RequestPlacedCompletionCancellation`(obligation)
- Guards:
  - `pending`
  - `not_requested`
- To: `Completed`

### `RequestPlacedCompletionCancellationReplayRunning`
- From: `Running`
- On: `RequestPlacedCompletionCancellation`(obligation)
- Guards:
  - `already_requested`
- To: `Running`

### `RequestPlacedCompletionCancellationReplayStopped`
- From: `Stopped`
- On: `RequestPlacedCompletionCancellation`(obligation)
- Guards:
  - `already_requested`
- To: `Stopped`

### `RequestPlacedCompletionCancellationReplayCompleted`
- From: `Completed`
- On: `RequestPlacedCompletionCancellation`(obligation)
- Guards:
  - `already_requested`
- To: `Completed`

### `RequestPlacedCompletionCancellationTerminalReplayRunning`
- From: `Running`
- On: `RequestPlacedCompletionCancellation`(obligation)
- Guards:
  - `not_pending`
  - `historical_or_resolved`
- To: `Running`

### `RequestPlacedCompletionCancellationTerminalReplayStopped`
- From: `Stopped`
- On: `RequestPlacedCompletionCancellation`(obligation)
- Guards:
  - `not_pending`
  - `historical_or_resolved`
- To: `Stopped`

### `RequestPlacedCompletionCancellationTerminalReplayCompleted`
- From: `Completed`
- On: `RequestPlacedCompletionCancellation`(obligation)
- Guards:
  - `not_pending`
  - `historical_or_resolved`
- To: `Completed`

### `RequestPlacedCompletionCancellationTerminalReplayDestroyed`
- From: `Destroyed`
- On: `RequestPlacedCompletionCancellation`(obligation)
- Guards:
  - `not_pending`
  - `historical_or_resolved`
- To: `Destroyed`

### `ResolvePlacedCompletionOutcomePendingRunning`
- From: `Running`
- On: `ResolvePlacedCompletionOutcome`(obligation)
- Guards:
  - `pending`
- To: `Running`

### `ResolvePlacedCompletionOutcomePendingStopped`
- From: `Stopped`
- On: `ResolvePlacedCompletionOutcome`(obligation)
- Guards:
  - `pending`
- To: `Stopped`

### `ResolvePlacedCompletionOutcomePendingCompleted`
- From: `Completed`
- On: `ResolvePlacedCompletionOutcome`(obligation)
- Guards:
  - `pending`
- To: `Completed`

### `ResolvePlacedCompletionOutcomeReplayRunning`
- From: `Running`
- On: `ResolvePlacedCompletionOutcome`(obligation)
- Guards:
  - `already_resolved`
- To: `Running`

### `ResolvePlacedCompletionOutcomeReplayStopped`
- From: `Stopped`
- On: `ResolvePlacedCompletionOutcome`(obligation)
- Guards:
  - `already_resolved`
- To: `Stopped`

### `ResolvePlacedCompletionOutcomeReplayCompleted`
- From: `Completed`
- On: `ResolvePlacedCompletionOutcome`(obligation)
- Guards:
  - `already_resolved`
- To: `Completed`

### `ResolvePlacedCompletionOutcomeHistoricalReplayRunning`
- From: `Running`
- On: `ResolvePlacedCompletionOutcome`(obligation)
- Guards:
  - `custody_absent`
  - `historical_sequence`
- To: `Running`

### `ResolvePlacedCompletionOutcomeHistoricalReplayStopped`
- From: `Stopped`
- On: `ResolvePlacedCompletionOutcome`(obligation)
- Guards:
  - `custody_absent`
  - `historical_sequence`
- To: `Stopped`

### `ResolvePlacedCompletionOutcomeHistoricalReplayCompleted`
- From: `Completed`
- On: `ResolvePlacedCompletionOutcome`(obligation)
- Guards:
  - `custody_absent`
  - `historical_sequence`
- To: `Completed`

### `ResolvePlacedCompletionOutcomeHistoricalReplayDestroyed`
- From: `Destroyed`
- On: `ResolvePlacedCompletionOutcome`(obligation)
- Guards:
  - `custody_absent`
  - `historical_sequence`
- To: `Destroyed`

### `ClosePlacedCompletionOutcomePendingRunning`
- From: `Running`
- On: `ClosePlacedCompletionOutcome`(obligation)
- Guards:
  - `pending`
- To: `Running`

### `ClosePlacedCompletionOutcomePendingStopped`
- From: `Stopped`
- On: `ClosePlacedCompletionOutcome`(obligation)
- Guards:
  - `pending`
- To: `Stopped`

### `ClosePlacedCompletionOutcomePendingCompleted`
- From: `Completed`
- On: `ClosePlacedCompletionOutcome`(obligation)
- Guards:
  - `pending`
- To: `Completed`

### `ClosePlacedCompletionOutcomeReplayRunning`
- From: `Running`
- On: `ClosePlacedCompletionOutcome`(obligation)
- Guards:
  - `custody_absent`
  - `historical_sequence`
- To: `Running`

### `ClosePlacedCompletionOutcomeReplayStopped`
- From: `Stopped`
- On: `ClosePlacedCompletionOutcome`(obligation)
- Guards:
  - `custody_absent`
  - `historical_sequence`
- To: `Stopped`

### `ClosePlacedCompletionOutcomeReplayCompleted`
- From: `Completed`
- On: `ClosePlacedCompletionOutcome`(obligation)
- Guards:
  - `custody_absent`
  - `historical_sequence`
- To: `Completed`

### `ClosePlacedCompletionOutcomeReplayDestroyed`
- From: `Destroyed`
- On: `ClosePlacedCompletionOutcome`(obligation)
- Guards:
  - `custody_absent`
  - `historical_sequence`
- To: `Destroyed`

### `AcknowledgePlacedCompletionOutcomePresentRunning`
- From: `Running`
- On: `AcknowledgePlacedCompletionOutcome`(obligation)
- Guards:
  - `resolved`
- To: `Running`

### `AcknowledgePlacedCompletionOutcomePresentStopped`
- From: `Stopped`
- On: `AcknowledgePlacedCompletionOutcome`(obligation)
- Guards:
  - `resolved`
- To: `Stopped`

### `AcknowledgePlacedCompletionOutcomePresentCompleted`
- From: `Completed`
- On: `AcknowledgePlacedCompletionOutcome`(obligation)
- Guards:
  - `resolved`
- To: `Completed`

### `AcknowledgePlacedCompletionOutcomeReplayRunning`
- From: `Running`
- On: `AcknowledgePlacedCompletionOutcome`(obligation)
- Guards:
  - `custody_absent`
  - `historical_sequence`
- To: `Running`

### `AcknowledgePlacedCompletionOutcomeReplayStopped`
- From: `Stopped`
- On: `AcknowledgePlacedCompletionOutcome`(obligation)
- Guards:
  - `custody_absent`
  - `historical_sequence`
- To: `Stopped`

### `AcknowledgePlacedCompletionOutcomeReplayCompleted`
- From: `Completed`
- On: `AcknowledgePlacedCompletionOutcome`(obligation)
- Guards:
  - `custody_absent`
  - `historical_sequence`
- To: `Completed`

### `AcknowledgePlacedCompletionOutcomeReplayDestroyed`
- From: `Destroyed`
- On: `AcknowledgePlacedCompletionOutcome`(obligation)
- Guards:
  - `custody_absent`
  - `historical_sequence`
- To: `Destroyed`

### `DisposePlacedCompletionOutcomeRunning`
- From: `Running`
- On: `DisposePlacedCompletionOutcome`(obligation)
- To: `Running`

### `DisposePlacedCompletionOutcomeStopped`
- From: `Stopped`
- On: `DisposePlacedCompletionOutcome`(obligation)
- To: `Stopped`

### `DisposePlacedCompletionOutcomeCompleted`
- From: `Completed`
- On: `DisposePlacedCompletionOutcome`(obligation)
- To: `Completed`

### `DisposePlacedCompletionOutcomeDestroyed`
- From: `Destroyed`
- On: `DisposePlacedCompletionOutcome`(obligation)
- To: `Destroyed`

### `GrantOperatorScopes`
- From: `Running`
- On: `GrantOperatorScopes`(principal, scopes, expires_at_ms)
- Guards:
  - `scopes_nonempty`
- Emits: `GrantRecorded`
- To: `Running`

### `RevokeOperatorScopesAll`
- From: `Running`
- On: `RevokeOperatorScopes`(principal, revoked, remaining)
- Guards:
  - `grant_present`
  - `revoked_nonempty`
  - `remaining_empty`
  - `revoked_are_granted`
  - `partition_covers_grant`
- Emits: `GrantRevoked`
- To: `Running`

### `RevokeOperatorScopesPartial`
- From: `Running`
- On: `RevokeOperatorScopes`(principal, revoked, remaining)
- Guards:
  - `grant_present`
  - `revoked_nonempty`
  - `remaining_nonempty`
  - `revoked_are_granted`
  - `remaining_are_granted`
  - `revoked_disjoint_from_remaining`
  - `partition_covers_grant`
- Emits: `GrantRevoked`
- To: `Running`

### `RevokeOperatorScopesAbsent`
- From: `Running`
- On: `RevokeOperatorScopes`(principal, revoked, remaining)
- Guards:
  - `grant_absent`
  - `revoked_empty`
  - `remaining_empty`
- To: `Running`

### `ResolveMemberOperatorAdmissionAdmittedRunning`
- From: `Running`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_matches`
  - `placement_present`
  - `requester_host_matches`
  - `host_binding_generation_matches`
  - `placed_carrier_binding_active`
- Emits: `MemberOperatorAdmitted`
- To: `Running`

### `ResolveMemberOperatorAdmissionAdmittedStopped`
- From: `Stopped`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_matches`
  - `placement_present`
  - `requester_host_matches`
  - `host_binding_generation_matches`
  - `placed_carrier_binding_active`
- Emits: `MemberOperatorAdmitted`
- To: `Stopped`

### `ResolveMemberOperatorAdmissionAdmittedCompleted`
- From: `Completed`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_matches`
  - `placement_present`
  - `requester_host_matches`
  - `host_binding_generation_matches`
  - `placed_carrier_binding_active`
- Emits: `MemberOperatorAdmitted`
- To: `Completed`

### `ResolveMemberOperatorAdmissionAdmittedDestroyed`
- From: `Destroyed`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_matches`
  - `placement_present`
  - `requester_host_matches`
  - `host_binding_generation_matches`
  - `placed_carrier_binding_active`
- Emits: `MemberOperatorAdmitted`
- To: `Destroyed`

### `ResolveMemberOperatorAdmissionUnknownIdentityRunning`
- From: `Running`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_unknown`
- Emits: `MemberOperatorRejected`
- To: `Running`

### `ResolveMemberOperatorAdmissionUnknownIdentityStopped`
- From: `Stopped`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_unknown`
- Emits: `MemberOperatorRejected`
- To: `Stopped`

### `ResolveMemberOperatorAdmissionUnknownIdentityCompleted`
- From: `Completed`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_unknown`
- Emits: `MemberOperatorRejected`
- To: `Completed`

### `ResolveMemberOperatorAdmissionUnknownIdentityDestroyed`
- From: `Destroyed`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_unknown`
- Emits: `MemberOperatorRejected`
- To: `Destroyed`

### `ResolveMemberOperatorAdmissionSenderKeyMismatchRunning`
- From: `Running`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Running`

### `ResolveMemberOperatorAdmissionSenderKeyMismatchStopped`
- From: `Stopped`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Stopped`

### `ResolveMemberOperatorAdmissionSenderKeyMismatchCompleted`
- From: `Completed`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Completed`

### `ResolveMemberOperatorAdmissionSenderKeyMismatchDestroyed`
- From: `Destroyed`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Destroyed`

### `ResolveMemberOperatorAdmissionStaleGenerationRunning`
- From: `Running`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Running`

### `ResolveMemberOperatorAdmissionStaleGenerationStopped`
- From: `Stopped`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Stopped`

### `ResolveMemberOperatorAdmissionStaleGenerationCompleted`
- From: `Completed`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Completed`

### `ResolveMemberOperatorAdmissionStaleGenerationDestroyed`
- From: `Destroyed`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Destroyed`

### `ResolveMemberOperatorAdmissionStaleFenceRunning`
- From: `Running`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Running`

### `ResolveMemberOperatorAdmissionStaleFenceStopped`
- From: `Stopped`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Stopped`

### `ResolveMemberOperatorAdmissionStaleFenceCompleted`
- From: `Completed`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Completed`

### `ResolveMemberOperatorAdmissionStaleFenceDestroyed`
- From: `Destroyed`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Destroyed`

### `ResolveMemberOperatorAdmissionStaleSessionRunning`
- From: `Running`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Running`

### `ResolveMemberOperatorAdmissionStaleSessionStopped`
- From: `Stopped`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Stopped`

### `ResolveMemberOperatorAdmissionStaleSessionCompleted`
- From: `Completed`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Completed`

### `ResolveMemberOperatorAdmissionStaleSessionDestroyed`
- From: `Destroyed`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Destroyed`

### `ResolveMemberOperatorAdmissionNoPlacementRunning`
- From: `Running`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_matches`
  - `placement_absent`
- Emits: `MemberOperatorRejected`
- To: `Running`

### `ResolveMemberOperatorAdmissionNoPlacementStopped`
- From: `Stopped`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_matches`
  - `placement_absent`
- Emits: `MemberOperatorRejected`
- To: `Stopped`

### `ResolveMemberOperatorAdmissionNoPlacementCompleted`
- From: `Completed`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_matches`
  - `placement_absent`
- Emits: `MemberOperatorRejected`
- To: `Completed`

### `ResolveMemberOperatorAdmissionNoPlacementDestroyed`
- From: `Destroyed`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_matches`
  - `placement_absent`
- Emits: `MemberOperatorRejected`
- To: `Destroyed`

### `ResolveMemberOperatorAdmissionStaleHostRunning`
- From: `Running`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_matches`
  - `placement_present`
  - `requester_host_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Running`

### `ResolveMemberOperatorAdmissionStaleHostStopped`
- From: `Stopped`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_matches`
  - `placement_present`
  - `requester_host_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Stopped`

### `ResolveMemberOperatorAdmissionStaleHostCompleted`
- From: `Completed`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_matches`
  - `placement_present`
  - `requester_host_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Completed`

### `ResolveMemberOperatorAdmissionStaleHostDestroyed`
- From: `Destroyed`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_matches`
  - `placement_present`
  - `requester_host_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Destroyed`

### `ResolveMemberOperatorAdmissionStaleHostBindingGenerationRunning`
- From: `Running`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_matches`
  - `placement_present`
  - `requester_host_matches`
  - `host_binding_generation_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Running`

### `ResolveMemberOperatorAdmissionStaleHostBindingGenerationStopped`
- From: `Stopped`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_matches`
  - `placement_present`
  - `requester_host_matches`
  - `host_binding_generation_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Stopped`

### `ResolveMemberOperatorAdmissionStaleHostBindingGenerationCompleted`
- From: `Completed`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_matches`
  - `placement_present`
  - `requester_host_matches`
  - `host_binding_generation_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Completed`

### `ResolveMemberOperatorAdmissionStaleHostBindingGenerationDestroyed`
- From: `Destroyed`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_matches`
  - `placement_present`
  - `requester_host_matches`
  - `host_binding_generation_mismatch`
- Emits: `MemberOperatorRejected`
- To: `Destroyed`

### `ResolveMemberOperatorAdmissionHostRevokedRunning`
- From: `Running`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_matches`
  - `placement_present`
  - `requester_host_matches`
  - `host_binding_generation_matches`
  - `placed_carrier_binding_inactive`
- Emits: `MemberOperatorRejected`
- To: `Running`

### `ResolveMemberOperatorAdmissionHostRevokedStopped`
- From: `Stopped`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_matches`
  - `placement_present`
  - `requester_host_matches`
  - `host_binding_generation_matches`
  - `placed_carrier_binding_inactive`
- Emits: `MemberOperatorRejected`
- To: `Stopped`

### `ResolveMemberOperatorAdmissionHostRevokedCompleted`
- From: `Completed`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_matches`
  - `placement_present`
  - `requester_host_matches`
  - `host_binding_generation_matches`
  - `placed_carrier_binding_inactive`
- Emits: `MemberOperatorRejected`
- To: `Completed`

### `ResolveMemberOperatorAdmissionHostRevokedDestroyed`
- From: `Destroyed`
- On: `ResolveMemberOperatorAdmission`(agent_identity, requester_generation, requester_fence_token, requester_host_id, requester_host_binding_generation, requester_member_session_id, sender_peer_id, request_id)
- Guards:
  - `identity_known`
  - `sender_key_matches`
  - `generation_matches`
  - `fence_matches`
  - `member_session_matches`
  - `placement_present`
  - `requester_host_matches`
  - `host_binding_generation_matches`
  - `placed_carrier_binding_inactive`
- Emits: `MemberOperatorRejected`
- To: `Destroyed`

### `ClassifyFlowStepDispatchLocalRunning`
- From: `Running`
- On: `ClassifyFlowStepDispatch`(run_id, step_id, target, overlay_present)
- Guards:
  - `placement_absent`
  - `not_overlay_autonomous`
- Emits: `FlowStepDispatchClassified`
- To: `Running`

### `ClassifyFlowStepDispatchLocalStopped`
- From: `Stopped`
- On: `ClassifyFlowStepDispatch`(run_id, step_id, target, overlay_present)
- Guards:
  - `placement_absent`
  - `not_overlay_autonomous`
- Emits: `FlowStepDispatchClassified`
- To: `Stopped`

### `ClassifyFlowStepDispatchLocalCompleted`
- From: `Completed`
- On: `ClassifyFlowStepDispatch`(run_id, step_id, target, overlay_present)
- Guards:
  - `placement_absent`
  - `not_overlay_autonomous`
- Emits: `FlowStepDispatchClassified`
- To: `Completed`

### `ClassifyFlowStepDispatchRemoteTurnDirectiveRunning`
- From: `Running`
- On: `ClassifyFlowStepDispatch`(run_id, step_id, target, overlay_present)
- Guards:
  - `placement_present`
  - `host_supports_tracked_turns`
  - `placed_carrier_binding_active`
  - `not_overlay_autonomous`
- Emits: `FlowStepDispatchClassified`
- To: `Running`

### `ClassifyFlowStepDispatchRemoteTurnDirectiveStopped`
- From: `Stopped`
- On: `ClassifyFlowStepDispatch`(run_id, step_id, target, overlay_present)
- Guards:
  - `placement_present`
  - `host_supports_tracked_turns`
  - `placed_carrier_binding_active`
  - `not_overlay_autonomous`
- Emits: `FlowStepDispatchClassified`
- To: `Stopped`

### `ClassifyFlowStepDispatchRemoteTurnDirectiveCompleted`
- From: `Completed`
- On: `ClassifyFlowStepDispatch`(run_id, step_id, target, overlay_present)
- Guards:
  - `placement_present`
  - `host_supports_tracked_turns`
  - `placed_carrier_binding_active`
  - `not_overlay_autonomous`
- Emits: `FlowStepDispatchClassified`
- To: `Completed`

### `ClassifyFlowStepDispatchRejectedOverlayAutonomousRunning`
- From: `Running`
- On: `ClassifyFlowStepDispatch`(run_id, step_id, target, overlay_present)
- Guards:
  - `overlay_autonomous`
- Emits: `FlowStepDispatchClassified`
- To: `Running`

### `ClassifyFlowStepDispatchRejectedOverlayAutonomousStopped`
- From: `Stopped`
- On: `ClassifyFlowStepDispatch`(run_id, step_id, target, overlay_present)
- Guards:
  - `overlay_autonomous`
- Emits: `FlowStepDispatchClassified`
- To: `Stopped`

### `ClassifyFlowStepDispatchRejectedOverlayAutonomousCompleted`
- From: `Completed`
- On: `ClassifyFlowStepDispatch`(run_id, step_id, target, overlay_present)
- Guards:
  - `overlay_autonomous`
- Emits: `FlowStepDispatchClassified`
- To: `Completed`

### `ClassifyFlowStepDispatchRejectedHostIncapableRunning`
- From: `Running`
- On: `ClassifyFlowStepDispatch`(run_id, step_id, target, overlay_present)
- Guards:
  - `placement_present`
  - `host_lacks_tracked_turn_support`
  - `placed_carrier_binding_active`
  - `not_overlay_autonomous`
- Emits: `FlowStepDispatchClassified`
- To: `Running`

### `ClassifyFlowStepDispatchRejectedHostIncapableStopped`
- From: `Stopped`
- On: `ClassifyFlowStepDispatch`(run_id, step_id, target, overlay_present)
- Guards:
  - `placement_present`
  - `host_lacks_tracked_turn_support`
  - `placed_carrier_binding_active`
  - `not_overlay_autonomous`
- Emits: `FlowStepDispatchClassified`
- To: `Stopped`

### `ClassifyFlowStepDispatchRejectedHostIncapableCompleted`
- From: `Completed`
- On: `ClassifyFlowStepDispatch`(run_id, step_id, target, overlay_present)
- Guards:
  - `placement_present`
  - `host_lacks_tracked_turn_support`
  - `placed_carrier_binding_active`
  - `not_overlay_autonomous`
- Emits: `FlowStepDispatchClassified`
- To: `Completed`

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
  - `placed_carrier_binding_active_or_local`
- Emits: `FlowTerminalized`
- To: `Running`

### `SubscribeAgentEventsRunning`
- From: `Running`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_live`
  - `session_bound`
  - `placement_absent`
- Emits: `AuthorizeAgentEventSubscription`
- To: `Running`

### `SubscribeAgentEventsStopped`
- From: `Stopped`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_live`
  - `session_bound`
  - `placement_absent`
- Emits: `AuthorizeAgentEventSubscription`
- To: `Stopped`

### `SubscribeAgentEventsCompleted`
- From: `Completed`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_live`
  - `session_bound`
  - `placement_absent`
- Emits: `AuthorizeAgentEventSubscription`
- To: `Completed`

### `SubscribeAgentEventsDestroyed`
- From: `Destroyed`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_live`
  - `session_bound`
  - `placement_absent`
- Emits: `AuthorizeAgentEventSubscription`
- To: `Destroyed`

### `SubscribeAgentEventsExternalRunning`
- From: `Running`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_live`
  - `session_bound`
  - `placement_present`
  - `placed_carrier_binding_active`
- Emits: `AuthorizeExternalAgentEventSubscription`
- To: `Running`

### `SubscribeAgentEventsExternalStopped`
- From: `Stopped`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_live`
  - `session_bound`
  - `placement_present`
  - `placed_carrier_binding_active`
- Emits: `AuthorizeExternalAgentEventSubscription`
- To: `Stopped`

### `SubscribeAgentEventsExternalCompleted`
- From: `Completed`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_live`
  - `session_bound`
  - `placement_present`
  - `placed_carrier_binding_active`
- Emits: `AuthorizeExternalAgentEventSubscription`
- To: `Completed`

### `SubscribeAgentEventsExternalDestroyed`
- From: `Destroyed`
- On: `SubscribeAgentEvents`(agent_identity)
- Guards:
  - `identity_present`
  - `runtime_live`
  - `session_bound`
  - `placement_present`
  - `placed_carrier_binding_active`
- Emits: `AuthorizeExternalAgentEventSubscription`
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
- On: `SubscribeAllAgentEvents`(session_bound_runtimes, external_members)
- Guards:
  - `session_bound_runtimes_match`
  - `external_members_match`
  - `external_carrier_bindings_active`
  - `session_bound_or_no_live_members`
- Emits: `AuthorizeAllAgentEventSubscription`
- To: `Running`

### `SubscribeAllAgentEventsStopped`
- From: `Stopped`
- On: `SubscribeAllAgentEvents`(session_bound_runtimes, external_members)
- Guards:
  - `session_bound_runtimes_match`
  - `external_members_match`
  - `external_carrier_bindings_active`
  - `session_bound_or_no_live_members`
- Emits: `AuthorizeAllAgentEventSubscription`
- To: `Stopped`

### `SubscribeAllAgentEventsCompleted`
- From: `Completed`
- On: `SubscribeAllAgentEvents`(session_bound_runtimes, external_members)
- Guards:
  - `session_bound_runtimes_match`
  - `external_members_match`
  - `external_carrier_bindings_active`
  - `session_bound_or_no_live_members`
- Emits: `AuthorizeAllAgentEventSubscription`
- To: `Completed`

### `SubscribeAllAgentEventsDestroyed`
- From: `Destroyed`
- On: `SubscribeAllAgentEvents`(session_bound_runtimes, external_members)
- Guards:
  - `session_bound_runtimes_match`
  - `external_members_match`
  - `external_carrier_bindings_active`
  - `session_bound_or_no_live_members`
- Emits: `AuthorizeAllAgentEventSubscription`
- To: `Destroyed`

### `SubscribeAllAgentEventsNoSessionBindingsRunning`
- From: `Running`
- On: `SubscribeAllAgentEvents`(session_bound_runtimes, external_members)
- Guards:
  - `session_bound_runtimes_match`
  - `external_members_match`
  - `no_session_bound_runtime`
  - `live_members_present`
- Emits: `RejectAllAgentEventSubscription`
- To: `Running`

### `SubscribeAllAgentEventsNoSessionBindingsStopped`
- From: `Stopped`
- On: `SubscribeAllAgentEvents`(session_bound_runtimes, external_members)
- Guards:
  - `session_bound_runtimes_match`
  - `external_members_match`
  - `no_session_bound_runtime`
  - `live_members_present`
- Emits: `RejectAllAgentEventSubscription`
- To: `Stopped`

### `SubscribeAllAgentEventsNoSessionBindingsCompleted`
- From: `Completed`
- On: `SubscribeAllAgentEvents`(session_bound_runtimes, external_members)
- Guards:
  - `session_bound_runtimes_match`
  - `external_members_match`
  - `no_session_bound_runtime`
  - `live_members_present`
- Emits: `RejectAllAgentEventSubscription`
- To: `Completed`

### `SubscribeAllAgentEventsNoSessionBindingsDestroyed`
- From: `Destroyed`
- On: `SubscribeAllAgentEvents`(session_bound_runtimes, external_members)
- Guards:
  - `session_bound_runtimes_match`
  - `external_members_match`
  - `no_session_bound_runtime`
  - `live_members_present`
- Emits: `RejectAllAgentEventSubscription`
- To: `Destroyed`

### `SubscribeMobEventsRunning`
- From: `Running`
- On: `SubscribeMobEvents`(initial_cursor, channel_capacity, poll_interval_ms, session_bound_runtimes, external_members)
- Guards:
  - `channel_capacity_positive`
  - `poll_interval_positive`
  - `session_bound_runtimes_match`
  - `external_members_match`
  - `external_carrier_bindings_active`
- Emits: `AuthorizeMobEventRouter`
- To: `Running`

### `SubscribeMobEventsStopped`
- From: `Stopped`
- On: `SubscribeMobEvents`(initial_cursor, channel_capacity, poll_interval_ms, session_bound_runtimes, external_members)
- Guards:
  - `channel_capacity_positive`
  - `poll_interval_positive`
  - `session_bound_runtimes_match`
  - `external_members_match`
  - `external_carrier_bindings_active`
- Emits: `AuthorizeMobEventRouter`
- To: `Stopped`

### `SubscribeMobEventsCompleted`
- From: `Completed`
- On: `SubscribeMobEvents`(initial_cursor, channel_capacity, poll_interval_ms, session_bound_runtimes, external_members)
- Guards:
  - `channel_capacity_positive`
  - `poll_interval_positive`
  - `session_bound_runtimes_match`
  - `external_members_match`
  - `external_carrier_bindings_active`
- Emits: `AuthorizeMobEventRouter`
- To: `Completed`

### `SubscribeMobEventsDestroyed`
- From: `Destroyed`
- On: `SubscribeMobEvents`(initial_cursor, channel_capacity, poll_interval_ms, session_bound_runtimes, external_members)
- Guards:
  - `channel_capacity_positive`
  - `poll_interval_positive`
  - `session_bound_runtimes_match`
  - `external_members_match`
  - `external_carrier_bindings_active`
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
- Guards:
  - `adaptive_lifecycle_drained`
- Emits: `EmitRunLifecycleNotice`
- To: `Stopped`

### `ShutdownStopped`
- From: `Stopped`
- On: `Shutdown`()
- Guards:
  - `adaptive_lifecycle_drained`
- Emits: `EmitRunLifecycleNotice`
- To: `Stopped`

### `ShutdownCompleted`
- From: `Completed`
- On: `Shutdown`()
- Guards:
  - `adaptive_lifecycle_drained`
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
- Guards:
  - `lifecycle_origin_open`
- Emits: `NotifyCoordinator`
- To: `Running`

### `BindCoordinatorRunning`
- From: `Running`
- On: `BindCoordinator`()
- Guards:
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
  - `known_run`
  - `start_run_command`
  - `run_pending`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandDispatchStep`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
  - `known_run`
  - `run_running`
  - `fail_step_command`
  - `has_step_id`
  - `failed_step_status`
  - `step_tracked`
  - `supervisor_escalation_due`
- Emits: `EmitRunLifecycleNotice`, `AppendFailureLedger`, `EscalateSupervisor`
- To: `Running`

### `AuthorizeFlowRunReducerCommandFailStepEscalationSuppressedByLifecycle`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `lifecycle_origin_closed`
  - `known_run`
  - `run_running`
  - `fail_step_command`
  - `has_step_id`
  - `failed_step_status`
  - `step_tracked`
  - `supervisor_escalation_due`
- Emits: `EmitRunLifecycleNotice`, `AppendFailureLedger`
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
  - `lifecycle_origin_open`
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

### `AuthorizeFlowRunReducerCommandProjectFrameStepStatusFailedEscalationSuppressedByLifecycle`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `lifecycle_origin_closed`
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
- Emits: `EmitRunLifecycleNotice`, `AppendFailureLedger`
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
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
  - `coordinator_bound`
- Emits: `EmitFlowRunNotice`
- To: `Running`

### `CreateRunRunning`
- From: `Running`
- On: `CreateRun`()
- Guards:
  - `lifecycle_origin_open`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `StartRunRunning`
- From: `Running`
- On: `StartRun`()
- Guards:
  - `lifecycle_origin_open`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `CompleteFlowRunning`
- From: `Running`
- On: `CompleteFlow`()
- Guards:
  - `active_runs_present`
- Emits: `FlowTerminalized`
- To: `Running`

### `CompleteFlowRunningZero`
- From: `Running`
- On: `CompleteFlow`()
- Guards:
  - `no_active_runs`
- Emits: `NotifyCoordinator`
- To: `Running`

### `FinishRunRunning`
- From: `Running`
- On: `FinishRun`()
- Guards:
  - `active_runs_present`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `FinishRunRunningZero`
- From: `Running`
- On: `FinishRun`()
- Guards:
  - `no_active_runs`
- Emits: `NotifyCoordinator`
- To: `Running`

### `CompleteFlowStopped`
- From: `Stopped`
- On: `CompleteFlow`()
- Guards:
  - `active_runs_present`
- Emits: `FlowTerminalized`
- To: `Stopped`

### `CompleteFlowStoppedZero`
- From: `Stopped`
- On: `CompleteFlow`()
- Guards:
  - `no_active_runs`
- Emits: `NotifyCoordinator`
- To: `Stopped`

### `CompleteFlowCompleted`
- From: `Completed`
- On: `CompleteFlow`()
- Guards:
  - `active_runs_present`
- Emits: `FlowTerminalized`
- To: `Completed`

### `CompleteFlowCompletedZero`
- From: `Completed`
- On: `CompleteFlow`()
- Guards:
  - `no_active_runs`
- Emits: `NotifyCoordinator`
- To: `Completed`

### `CompleteFlowDestroyed`
- From: `Destroyed`
- On: `CompleteFlow`()
- Guards:
  - `active_runs_present`
- Emits: `FlowTerminalized`
- To: `Destroyed`

### `CompleteFlowDestroyedZero`
- From: `Destroyed`
- On: `CompleteFlow`()
- Guards:
  - `no_active_runs`
- Emits: `NotifyCoordinator`
- To: `Destroyed`

### `FinishRunStopped`
- From: `Stopped`
- On: `FinishRun`()
- Guards:
  - `active_runs_present`
- Emits: `EmitRunLifecycleNotice`
- To: `Stopped`

### `FinishRunStoppedZero`
- From: `Stopped`
- On: `FinishRun`()
- Guards:
  - `no_active_runs`
- Emits: `NotifyCoordinator`
- To: `Stopped`

### `FinishRunCompleted`
- From: `Completed`
- On: `FinishRun`()
- Guards:
  - `active_runs_present`
- Emits: `EmitRunLifecycleNotice`
- To: `Completed`

### `FinishRunCompletedZero`
- From: `Completed`
- On: `FinishRun`()
- Guards:
  - `no_active_runs`
- Emits: `NotifyCoordinator`
- To: `Completed`

### `FinishRunDestroyed`
- From: `Destroyed`
- On: `FinishRun`()
- Guards:
  - `active_runs_present`
- Emits: `EmitRunLifecycleNotice`
- To: `Destroyed`

### `FinishRunDestroyedZero`
- From: `Destroyed`
- On: `FinishRun`()
- Guards:
  - `no_active_runs`
- Emits: `NotifyCoordinator`
- To: `Destroyed`

### `ClassifyRetirePendingSpawnDispositionCancelCommittedRunning`
- From: `Running`
- On: `ClassifyRetirePendingSpawnDisposition`(agent_identity)
- Guards:
  - `retire_identity_is_committed`
  - `retire_generation_is_committed`
  - `retire_pending_spawn_is_present`
- Emits: `RetirePendingSpawnCancellationAuthorized`
- To: `Running`

### `ClassifyRetirePendingSpawnDispositionCancelCommittedStopped`
- From: `Stopped`
- On: `ClassifyRetirePendingSpawnDisposition`(agent_identity)
- Guards:
  - `retire_identity_is_committed`
  - `retire_generation_is_committed`
  - `retire_pending_spawn_is_present`
- Emits: `RetirePendingSpawnCancellationAuthorized`
- To: `Stopped`

### `ClassifyRetirePendingSpawnDispositionCommittedWithoutPendingRunning`
- From: `Running`
- On: `ClassifyRetirePendingSpawnDisposition`(agent_identity)
- Guards:
  - `retire_identity_is_committed`
  - `retire_generation_is_committed`
  - `retire_pending_spawn_is_absent`
- Emits: `RetireCommittedIncarnationWithoutPendingSpawnResolved`
- To: `Running`

### `ClassifyRetirePendingSpawnDispositionCommittedWithoutPendingStopped`
- From: `Stopped`
- On: `ClassifyRetirePendingSpawnDisposition`(agent_identity)
- Guards:
  - `retire_identity_is_committed`
  - `retire_generation_is_committed`
  - `retire_pending_spawn_is_absent`
- Emits: `RetireCommittedIncarnationWithoutPendingSpawnResolved`
- To: `Stopped`

### `ClassifyRetirePendingSpawnDispositionPreserveAbsentRunning`
- From: `Running`
- On: `ClassifyRetirePendingSpawnDisposition`(agent_identity)
- Guards:
  - `retire_identity_is_absent`
- Emits: `RetireAbsentPendingSpawnPreservationResolved`
- To: `Running`

### `ClassifyRetirePendingSpawnDispositionPreserveAbsentStopped`
- From: `Stopped`
- On: `ClassifyRetirePendingSpawnDisposition`(agent_identity)
- Guards:
  - `retire_identity_is_absent`
- Emits: `RetireAbsentPendingSpawnPreservationResolved`
- To: `Stopped`

### `RetireRunningReleasing`
- From: `Running`
- On: `Retire`(mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id)
- Guards:
  - `placement_absent`
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

### `RetireRemoteReleasingRunning`
- From: `Running`
- On: `Retire`(mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id)
- Guards:
  - `placement_present`
  - `placed_carrier_binding_active`
  - `active_members_present`
  - `runtime_id_present`
  - `identity_binding_matches`
  - `generation_matches`
  - `prior_session_binding_present`
  - `releasing_present`
  - `releasing_matches_current`
  - `session_matches_releasing`
- Emits: `AppendLifecycleJournal`, `RequestMemberRelease`, `RequestKickoffQuiesce`
- To: `Running`

### `RetireRemoteConfirmedRevokedRunning`
- From: `Running`
- On: `Retire`(mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id)
- Guards:
  - `placement_present`
  - `placed_carrier_binding_confirmed_revoked`
  - `active_members_present`
  - `runtime_id_present`
  - `identity_binding_matches`
  - `generation_matches`
  - `prior_session_binding_present`
  - `releasing_present`
  - `releasing_matches_current`
  - `session_matches_releasing`
- Emits: `AppendLifecycleJournal`, `RequestKickoffQuiesce`
- To: `Running`

### `RetireRunningPreservingBinding`
- From: `Running`
- On: `Retire`(mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id)
- Guards:
  - `placement_absent`
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
  - `retirement_session_absent`
  - `releasing_absent`
  - `session_absent`
- Emits: `AppendLifecycleJournal`, `RequestKickoffQuiesce`
- To: `Running`

### `RetireStoppedReleasing`
- From: `Stopped`
- On: `Retire`(mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id)
- Guards:
  - `placement_absent`
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

### `RetireRemoteReleasingStopped`
- From: `Stopped`
- On: `Retire`(mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id)
- Guards:
  - `placement_present`
  - `placed_carrier_binding_active`
  - `active_members_present`
  - `runtime_id_present`
  - `identity_binding_matches`
  - `generation_matches`
  - `prior_session_binding_present`
  - `releasing_present`
  - `releasing_matches_current`
  - `session_matches_releasing`
- Emits: `AppendLifecycleJournal`, `RequestMemberRelease`, `RequestKickoffQuiesce`
- To: `Stopped`

### `RetireRemoteConfirmedRevokedStopped`
- From: `Stopped`
- On: `Retire`(mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id)
- Guards:
  - `placement_present`
  - `placed_carrier_binding_confirmed_revoked`
  - `active_members_present`
  - `runtime_id_present`
  - `identity_binding_matches`
  - `generation_matches`
  - `prior_session_binding_present`
  - `releasing_present`
  - `releasing_matches_current`
  - `session_matches_releasing`
- Emits: `AppendLifecycleJournal`, `RequestKickoffQuiesce`
- To: `Stopped`

### `RetireStoppedPreservingBinding`
- From: `Stopped`
- On: `Retire`(mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id)
- Guards:
  - `placement_absent`
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
  - `retirement_session_absent`
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
- Guards:
  - `adaptive_lifecycle_drained`
- Emits: `EmitMemberLifecycleNotice`
- To: `Running`

### `RetireAllStopped`
- From: `Stopped`
- On: `RetireAll`()
- Guards:
  - `adaptive_lifecycle_drained`
- Emits: `EmitMemberLifecycleNotice`
- To: `Stopped`

### `RetireAllCompleted`
- From: `Completed`
- On: `RetireAll`()
- Guards:
  - `adaptive_lifecycle_drained`
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
  - `adaptive_lifecycle_drained`
  - `session_ingress_detaches_closed`
  - `placed_completion_quiesce_started`
  - `placed_completion_destroy_intent`
  - `remote_turn_pending_drained`
  - `remote_turn_committed_drained`
  - `remote_turn_resolved_drained`
  - `placed_completion_pending_drained`
  - `placed_completion_cancel_requested_drained`
  - `placed_completion_resolved_drained`
  - `placed_kickoff_pending_drained`
  - `placed_kickoff_resolved_drained`
  - `host_bind_phases_drained`
  - `replacement_host_bind_requests_drained`
  - `bound_hosts_drained`
  - `host_authority_epochs_drained`
  - `host_public_keys_drained`
  - `host_endpoints_drained`
  - `host_live_endpoints_drained`
  - `placed_carrier_cleanup_drained`
  - `pending_placed_spawns_drained`
  - `committed_placed_spawns_drained`
  - `kickoff_waiters_quiesced`
  - `pending_spawns_drained`
- To: `Destroyed`

### `RespawnRunning`
- From: `Running`
- On: `Respawn`(agent_runtime_id)
- Guards:
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
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
  - `lifecycle_origin_open`
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
- `mob_actor_authority` (machine `MobMachine`): `meerkat-mob/src/runtime/actor.rs` — MobMachine actor authority and command execution for wire, unwire, spawn, ensure member, reconcile, observe runtime, submit work, retire, recover durable incarnations, complete, mark completed, stop/stopped, resume, force cancel, subscribe events, shutdown, destroy, terminalized member, record operator action provenance, flow, run, create frame seed, create loop seed, project frame phase, project loop state, orchestrator, coordinator, cleanup, append failure ledger, escalate supervisor, peer, progress, notices, kickoff pending/replay and resolve started/callback pending/failed/clear, wiring graph, and session binding
- `mob_owner_bridge_cleanup_authority` (machine `MobMachine`): `meerkat-mob-mcp/src/lib.rs` — MobMachine owner bridge session cleanup authority for owner bridge cleanup requires owner and implicit delegation requires owner invariants
- `mob_coordination_board_authority` (machine `MobMachine`): `meerkat-mob/src/coordination.rs` — MobMachine coordination board authority: record work intent, record resource claim, update coordination work intent status planned active blocked completed cancelled, update coordination resource claim status active released expired cancelled, observe coordination resource claim overlap, and the recorded/status-changed/overlap-observed coordination effects
- `mob_operator_admission_authority` (machine `MobMachine`): `meerkat-mob-mcp/src/agent_tools.rs` — MobMachine operator-admission authority for the mob tool surface: resolve create mob admission from the create-mobs capability observation and resolve profile mutation admission from the mutate-profiles capability observation, emitting the create-mob and profile-mutation admission resolved verdicts the surface mirrors (denied -> access denied)
- `mob_membership_classifier_authority` (machine `MobMachine`): `meerkat-mob/src/runtime/actor.rs` — MobMachine membership and runtime-incarnation classifiers owned by the actor: probe member admission duplicate or admitted from machine-owned binding and pending-spawn state; compute respawn generation successor; reconcile desired members to spawn retain or retire against current bindings emitting member spawn required, member retain required, and member retire required; set and observe external member rebind capability available or unavailable; classify turn timeout disposition detached canceled or retryable; and seed orphan budget once at startup, emitting the member admission probed, respawn generation computed, external member rebind capability, and turn timeout disposition classified effects
- `mob_flow_fault_topology_escalation_authority` (machine `MobMachine`): `meerkat-mob/src/runtime/flow.rs` — MobMachine flow-step fault, topology-edge, and supervisor-escalation classifiers owned by the flow engine: classify step output fault retry or terminal malformed json into a step fault disposition; evaluate topology edge rule allow deny or default into a policy decision verdict; and escalate to supervisor target found with a real supervisor identity or no eligible target, emitting the step output fault classified, topology edge verdict resolved, supervisor escalation requested, and supervisor escalation failed effects

### Scenarios
- `coordination-board-records-and-overlap` — record coordination work intent and resource claim, update coordination work intent and resource claim status across planned active blocked completed cancelled released expired, and observe coordination resource claim overlap with recomputed revision and event sequence
- `spawn-work-terminal` — member spawn, ensure member, reconcile, runtime-ready observation, work submission, and terminal work closure
- `retire-recover-destroy` — member retires, durable incarnation recovery preserves monotone identity history, stops/stopped, resumes, shuts down, and destroys cleanly
- `wiring-and-session-binding` — wire and unwire members, enforce known identity for session bindings, expose pending spawn, member session binding changed, and wiring lifecycle notices
- `flow-and-run-lifecycle` — run flow, start flow, create run, create frame seed, create loop seed, project frame phase, project loop state, start run, complete flow, finish run, mark completed, kickoff resolve started or failed, kickoff clear, flow terminalized, and force cancel running work
- `event-subscriptions-and-notices` — subscribe agent, all agent, and mob events; emit member, run, flow, progress, terminal, and wiring notices
- `orchestrator-coordinator-cleanup` — initialize, stop, resume, and destroy orchestrator; bind or unbind coordinator; begin and finish cleanup; notify coordinator and escalate supervisor
- `owner-bridge-cleanup` — bind owner bridge session, owner bridge cleanup requires owner, implicit delegation requires owner, and recover owner bridge session authority for archive cleanup
- `operator-provenance-and-peer-input` — record operator action provenance, trust operation peer, admit peer input, append failure ledger, surface peer-exposed member inputs, and resolve operator create mob admission and profile mutation admission verdicts the tool surface mirrors
- `membership-admission-respawn-reconcile-rebind-timeout` — probe member admission duplicate or admitted, compute respawn generation, reconcile desired members to spawn retain or retire emitting member spawn required member retain required and member retire required, set and observe external member rebind capability available or unavailable, classify turn timeout disposition detached canceled or retryable, and seed orphan budget
- `flow-fault-topology-supervisor-escalation` — classify step output fault retry or terminal malformed json into a step fault disposition, evaluate topology edge rule allow deny or default into a policy decision verdict resolved, and escalate to supervisor target found with eligible supervisor identity or no eligible target emitting supervisor escalation requested or failed
