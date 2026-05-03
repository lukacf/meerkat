# MobMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `2`
- Rust owner: `self` / `catalog::dsl::mob_machine`

## State
- Phase enum: `Running | Stopped | Completed | Destroyed`
- `live_runtime_ids`: `Set<AgentRuntimeId>`
- `externally_addressable_runtime_ids`: `Set<AgentRuntimeId>`
- `runtime_fence_tokens`: `Map<AgentRuntimeId, FenceToken>`
- `active_run_count`: `u64`
- `run_status`: `Map<RunId, FlowRunStatus>`
- `run_ordered_steps`: `Map<RunId, Seq<StepId>>`
- `run_tracked_steps`: `Map<RunId, Set<StepId>>`
- `run_step_status`: `Map<RunId, Map<StepId, Option<StepRunStatus>>>`
- `run_step_status_flat`: `Map<RunStepKey, StepRunStatus>`
- `run_output_recorded`: `Map<RunId, Map<StepId, Bool>>`
- `run_step_condition_results_flat`: `Map<RunStepKey, Option<Bool>>`
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
- `run_output_recorded_flat`: `Map<RunStepKey, Bool>`
- `run_step_target_counts_flat`: `Map<RunStepKey, u64>`
- `run_step_target_success_counts_flat`: `Map<RunStepKey, u64>`
- `run_step_target_terminal_failure_counts_flat`: `Map<RunStepKey, u64>`
- `run_target_retry_counts`: `Map<RunId, Map<String, u64>>`
- `run_target_retry_counts_flat`: `Map<RunStepKey, u64>`
- `run_failure_count`: `Map<RunId, u64>`
- `run_consecutive_failure_count`: `Map<RunId, u64>`
- `run_escalation_threshold`: `Map<RunId, u32>`
- `run_max_step_retries`: `Map<RunId, u32>`
- `run_ready_frames`: `Map<RunId, Seq<FrameId>>`
- `run_ready_frame_membership`: `Map<RunId, Set<FrameId>>`
- `run_ready_frame_membership_flat`: `Set<FrameId>`
- `run_pending_body_frame_loops`: `Map<RunId, Seq<LoopInstanceId>>`
- `run_pending_body_frame_loop_membership`: `Map<RunId, Set<LoopInstanceId>>`
- `run_pending_body_frame_loop_membership_flat`: `Set<LoopInstanceId>`
- `run_active_node_count`: `Map<RunId, u64>`
- `run_active_frame_count`: `Map<RunId, u64>`
- `run_last_granted_frame`: `Map<RunId, FrameId>`
- `run_last_granted_loop`: `Map<RunId, LoopInstanceId>`
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
- `frame_output_recorded_flat`: `Map<FrameNodeKey, Bool>`
- `frame_last_admitted_node`: `Map<FrameId, FlowNodeId>`
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
- `member_kickoff_pending`: `Set<String>`
- `member_kickoff_starting`: `Set<String>`
- `member_kickoff_callback_pending`: `Set<String>`
- `member_kickoff_started`: `Set<String>`
- `member_kickoff_failed`: `Set<String>`
- `member_kickoff_cancelled`: `Set<String>`
- `member_kickoff_error`: `Map<String, String>`
- `member_restore_failures`: `Map<AgentIdentity, String>`
- `member_state_markers`: `Map<AgentRuntimeId, MobMemberState>`
- `wiring_edges`: `Set<WiringEdge>`
- `external_peer_edges`: `Set<ExternalPeerEdge>`
- `identity_to_runtime`: `Map<AgentIdentity, AgentRuntimeId>`
- `tasks`: `Map<TaskId, MobTask>`
- `in_progress_task_ids`: `Set<TaskId>`
- `completed_task_ids`: `Set<TaskId>`
- `member_session_bindings`: `Map<AgentIdentity, SessionId>`
- `pending_session_ingress_detach_runtime_ids`: `Set<AgentRuntimeId>`
- `topology_epoch`: `u64`

## Inputs
- `RunFlow`(run_id: RunId, step_ids: Set<StepId>, ordered_steps: Seq<StepId>, step_has_conditions: Map<StepId, Bool>, step_dependencies: Map<StepId, Seq<StepId>>, step_dependency_modes: Map<StepId, DependencyMode>, step_branches: Map<StepId, Option<BranchId>>, step_collection_policies: Map<StepId, CollectionPolicyKind>, step_quorum_thresholds: Map<StepId, u32>, escalation_threshold: u32, max_step_retries: u32, max_active_nodes: u64, max_active_frames: u64, max_frame_depth: u64)
- `CreateRunSeed`(run_id: RunId, step_ids: Set<StepId>, ordered_steps: Seq<StepId>, step_has_conditions: Map<StepId, Bool>, step_dependencies: Map<StepId, Seq<StepId>>, step_dependency_modes: Map<StepId, DependencyMode>, step_branches: Map<StepId, Option<BranchId>>, step_collection_policies: Map<StepId, CollectionPolicyKind>, step_quorum_thresholds: Map<StepId, u32>, escalation_threshold: u32, max_step_retries: u32, max_active_nodes: u64, max_active_frames: u64, max_frame_depth: u64)
- `CreateFrameSeed`(run_id: RunId, frame_id: FrameId, frame_scope: FrameScope, loop_instance_id: Option<LoopInstanceId>, iteration: u32, tracked_nodes: Set<FlowNodeId>, ordered_nodes: Seq<FlowNodeId>, node_kind: Map<FlowNodeId, FlowNodeKind>, node_dependencies: Map<FlowNodeId, Seq<FlowNodeId>>, node_dependency_modes: Map<FlowNodeId, DependencyMode>, node_branches: Map<FlowNodeId, Option<BranchId>>, node_step_ids: Map<FlowNodeId, StepId>, node_loop_ids: Map<FlowNodeId, LoopId>, node_status: Map<FlowNodeId, NodeRunStatus>, ready_queue: Seq<FlowNodeId>)
- `CreateLoopSeed`(loop_instance_id: LoopInstanceId, parent_frame_id: FrameId, parent_node_id: FlowNodeId, loop_id: LoopId, depth: u32, max_iterations: u64)
- `RecordLoopBodyFrameCompleted`(loop_instance_id: LoopInstanceId, iteration: u64)
- `RecordLoopUntilConditionMet`(loop_instance_id: LoopInstanceId, iteration: u64)
- `RecordLoopUntilConditionFailed`(loop_instance_id: LoopInstanceId, iteration: u64)
- `AuthorizeFlowRunReducerCommand`(run_id: RunId, command: FlowRunReducerCommandKind, step_id: Option<StepId>, run_step_key: Option<RunStepKey>, step_status: Option<StepRunStatus>, target_count: Option<u64>, frame_id: Option<FrameId>, node_id: Option<FlowNodeId>, loop_instance_id: Option<LoopInstanceId>, retry_key: Option<String>)
- `AuthorizeFlowFrameReducerCommand`(frame_id: FrameId, command: FlowFrameReducerCommandKind, node_id: Option<FlowNodeId>, frame_node_key: Option<FrameNodeKey>, node_status: Option<NodeRunStatus>, terminal_status: Option<FrameStatus>)
- `AuthorizeLoopIterationReducerCommand`(loop_instance_id: LoopInstanceId, command: LoopIterationReducerCommandKind, body_frame_id: Option<FrameId>, body_frame_iteration: Option<u64>)
- `CancelFlow`(run_id: RunId)
- `FlowStatus`
- `Spawn`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, external_addressable: Bool, bridge_session_id: SessionId, replacing: Option<SessionId>)
- `EnsureMember`(agent_identity: AgentIdentity)
- `Reconcile`(desired: Set<AgentIdentity>, retire_stale: Bool)
- `Retire`(mob_id: MobId, agent_runtime_id: AgentRuntimeId, agent_identity: AgentIdentity, releasing: Option<SessionId>, session_id: SessionId)
- `Respawn`(agent_runtime_id: AgentRuntimeId)
- `RetireAll`
- `WireMembers`(edge: WiringEdge)
- `UnwireMembers`(edge: WiringEdge)
- `WireExternalPeer`(edge: ExternalPeerEdge)
- `UnwireExternalPeer`(edge: ExternalPeerEdge)
- `SessionIngressDetachedForMobDestroy`(mob_id: MobId, agent_runtime_id: AgentRuntimeId)
- `SessionIngressDetachFailedForMobDestroy`(mob_id: MobId, agent_runtime_id: AgentRuntimeId, reason: String)
- `SubmitWork`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId, origin: WorkOrigin)
- `CancelWork`(work_id: WorkId)
- `CancelAllWork`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `Stop`
- `Resume`
- `Complete`
- `Reset`
- `Destroy`
- `TaskCreate`(task_id: TaskId, task_payload: MobTask)
- `TaskUpdate`(task_id: TaskId, new_status: TaskStatus)
- `TaskList`
- `TaskGet`
- `RosterSnapshot`
- `ListMembers`
- `ListMembersIncludingRetiring`
- `ListAllMembers`
- `MemberStatus`
- `SubscribeAgentEvents`
- `SubscribeAllAgentEvents`
- `SubscribeMobEvents`
- `PollEvents`
- `ReplayAllEvents`
- `RecordOperatorActionProvenance`
- `GetMember`
- `SetSpawnPolicy`
- `Shutdown`
- `ForceCancel`
- `KickoffMarkPending`(member_id: String)
- `KickoffMarkStarting`(member_id: String)
- `StartupMarkReady`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `KickoffResolveStarted`(member_id: String)
- `KickoffResolveCallbackPending`(member_id: String)
- `KickoffResolveFailed`(member_id: String, error: String)
- `KickoffCancelRequested`(member_id: String)
- `KickoffClear`(member_id: String)

## Surface-only Inputs
- `FlowStatus`
- `TaskList`
- `TaskGet`
- `RosterSnapshot`
- `ListMembers`
- `ListMembersIncludingRetiring`
- `ListAllMembers`
- `MemberStatus`
- `CancelWork`
- `PollEvents`
- `ReplayAllEvents`
- `GetMember`

## Signals
- `ObserveRuntimeReady`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `RetireMember`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, session_id: SessionId)
- `ObserveRuntimeRetired`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `ResetMember`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, external_addressable: Bool, session_id: SessionId)
- `RespawnMember`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, external_addressable: Bool, session_id: SessionId)
- `DestroyMob`(session_id: SessionId)
- `ObserveRuntimeDestroyed`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
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
- `RequestRuntimeBinding`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, session_id: SessionId)
- `RequestRuntimeIngress`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId, origin: WorkOrigin)
- `RequestRuntimeRetire`(session_id: SessionId)
- `RequestRuntimeDestroy`(session_id: SessionId)
- `RequestSessionIngressDetachForMobDestroy`(mob_id: MobId, agent_runtime_id: AgentRuntimeId)
- `EmitMemberLifecycleNotice`(kind: MemberLifecycleKind)
- `EmitRunLifecycleNotice`
- `EmitFlowRunNotice`
- `AppendFailureLedger`
- `FlowTerminalized`
- `EscalateSupervisor`
- `NotifyCoordinator`
- `ExposePendingSpawn`
- `EmitMemberTerminalNotice`
- `AdmitPeerInput`
- `EmitProgressNote`
- `EmitTaskNotice`
- `PersistKickoffUpdate`(member_id: String, phase: KickoffPhase)
- `PersistKickoffFailureUpdate`(member_id: String, phase: KickoffPhase, error: String)
- `EmitKickoffLifecycleNotice`(member_id: String, intent: KickoffIntent)
- `WiringGraphChanged`(epoch: u64)
- `MemberSessionBindingChanged`(epoch: u64, agent_identity: AgentIdentity, old_session_id: Option<SessionId>, new_session_id: Option<SessionId>)
- `EmitWiringLifecycleNotice`(kind: WiringLifecycleKind, edge: WiringEdge)
- `EmitExternalPeerWiringLifecycleNotice`(kind: WiringLifecycleKind, edge: ExternalPeerEdge)

## Invariants
- `bindings_require_known_identity`

## Transitions
### `SpawnRunningFresh`
- From: `Running`
- On: `Spawn`(agent_identity, agent_runtime_id, fence_token, generation, external_addressable, bridge_session_id, replacing)
- Guards:
  - `coordinator_bound`
  - `no_prior_session_binding`
  - `replacing_absent`
- Emits: `RequestRuntimeBinding`, `MemberSessionBindingChanged`, `EmitMemberLifecycleNotice`
- To: `Running`

### `SpawnRunningReplacing`
- From: `Running`
- On: `Spawn`(agent_identity, agent_runtime_id, fence_token, generation, external_addressable, bridge_session_id, replacing)
- Guards:
  - `coordinator_bound`
  - `prior_session_binding_present`
  - `replacing_present`
  - `replacing_matches_current`
- Emits: `RequestRuntimeBinding`, `MemberSessionBindingChanged`, `EmitMemberLifecycleNotice`
- To: `Running`

### `EnsureMemberRunningExisting`
- From: `Running`
- On: `EnsureMember`(agent_identity)
- Guards:
  - `coordinator_bound`
  - `identity_present`
- To: `Running`

### `EnsureMemberRunningMissing`
- From: `Running`
- On: `EnsureMember`(agent_identity)
- Guards:
  - `coordinator_bound`
  - `identity_absent`
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

### `ObserveRuntimeReady`
- From: `Running`
- On: `ObserveRuntimeReady`(agent_runtime_id, fence_token)
- Guards:
  - `current_binding_matches`
  - `fence_token_present`
- To: `Running`

### `StartupMarkReadyRunning`
- From: `Running`
- On: `StartupMarkReady`(agent_runtime_id, fence_token)
- Guards:
  - `current_binding_matches`
  - `fence_token_present`
- To: `Running`

### `StartupMarkReadyStopped`
- From: `Stopped`
- On: `StartupMarkReady`(agent_runtime_id, fence_token)
- Guards:
  - `current_binding_matches`
  - `fence_token_present`
- To: `Stopped`

### `StartupMarkReadyCompleted`
- From: `Completed`
- On: `StartupMarkReady`(agent_runtime_id, fence_token)
- Guards:
  - `current_binding_matches`
  - `fence_token_present`
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

### `SubmitWorkRunningExternal`
- From: `Running`
- On: `SubmitWork`(agent_runtime_id, fence_token, work_id, origin)
- Guards:
  - `active_members_present`
  - `current_binding_matches`
  - `external_origin`
  - `runtime_externally_addressable`
- Emits: `RequestRuntimeIngress`
- To: `Running`

### `SubmitWorkRunningInternal`
- From: `Running`
- On: `SubmitWork`(agent_runtime_id, fence_token, work_id, origin)
- Guards:
  - `active_members_present`
  - `current_binding_matches`
  - `internal_origin`
- Emits: `RequestRuntimeIngress`
- To: `Running`

### `RetireMember`
- From: `Running`
- On: `RetireMember`(agent_runtime_id, fence_token, session_id)
- Guards:
  - `current_binding_matches`
  - `fence_token_present`
- Emits: `RequestRuntimeRetire`
- To: `Running`

### `ObserveRuntimeRetired`
- From: `Running`
- On: `ObserveRuntimeRetired`(agent_runtime_id, fence_token)
- Guards:
  - `current_binding_matches`
  - `fence_token_present`
- Emits: `EmitMemberLifecycleNotice`
- To: `Running`

### `ResetMember`
- From: `Running`, `Stopped`
- On: `ResetMember`(agent_identity, agent_runtime_id, fence_token, generation, external_addressable, session_id)
- Emits: `RequestRuntimeBinding`, `EmitMemberLifecycleNotice`
- To: `Running`

### `RespawnMember`
- From: `Running`
- On: `RespawnMember`(agent_identity, agent_runtime_id, fence_token, generation, external_addressable, session_id)
- Emits: `RequestRuntimeBinding`, `EmitMemberLifecycleNotice`
- To: `Running`

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
- Emits: `RequestRuntimeDestroy`
- To: `Destroyed`

### `ObserveRuntimeDestroyed`
- From: `Running`, `Stopped`, `Completed`, `Destroyed`
- On: `ObserveRuntimeDestroyed`(agent_runtime_id, fence_token)
- Guards:
  - `current_binding_matches`
  - `fence_token_present`
- Emits: `EmitMemberLifecycleNotice`
- To: `Destroyed`

### `RecordOperatorActionProvenanceRunning`
- From: `Running`
- On: `RecordOperatorActionProvenance`()
- To: `Running`

### `RecordOperatorActionProvenanceStopped`
- From: `Stopped`
- On: `RecordOperatorActionProvenance`()
- To: `Stopped`

### `RecordOperatorActionProvenanceCompleted`
- From: `Completed`
- On: `RecordOperatorActionProvenance`()
- To: `Completed`

### `RecordOperatorActionProvenanceDestroyed`
- From: `Destroyed`
- On: `RecordOperatorActionProvenance`()
- To: `Destroyed`

### `SetSpawnPolicyRunning`
- From: `Running`
- On: `SetSpawnPolicy`()
- To: `Running`

### `SetSpawnPolicyStopped`
- From: `Stopped`
- On: `SetSpawnPolicy`()
- To: `Stopped`

### `SetSpawnPolicyCompleted`
- From: `Completed`
- On: `SetSpawnPolicy`()
- To: `Completed`

### `SetSpawnPolicyDestroyed`
- From: `Destroyed`
- On: `SetSpawnPolicy`()
- To: `Destroyed`

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
- Emits: `EmitRunLifecycleNotice`
- To: `Completed`

### `ResetToRunning`
- From: `Running`, `Stopped`, `Completed`
- On: `Reset`()
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `WireMembersRunning`
- From: `Running`
- On: `WireMembers`(edge)
- Guards:
  - `edge_not_already_wired`
- Emits: `WiringGraphChanged`, `EmitWiringLifecycleNotice`
- To: `Running`

### `UnwireMembersRunning`
- From: `Running`
- On: `UnwireMembers`(edge)
- Guards:
  - `edge_currently_wired`
- Emits: `WiringGraphChanged`, `EmitWiringLifecycleNotice`
- To: `Running`

### `WireExternalPeerRunning`
- From: `Running`
- On: `WireExternalPeer`(edge)
- Guards:
  - `external_peer_not_already_wired`
- Emits: `WiringGraphChanged`, `EmitExternalPeerWiringLifecycleNotice`
- To: `Running`

### `UnwireExternalPeerRunning`
- From: `Running`
- On: `UnwireExternalPeer`(edge)
- Guards:
  - `external_peer_currently_wired`
- Emits: `WiringGraphChanged`, `EmitExternalPeerWiringLifecycleNotice`
- To: `Running`

### `TaskCreateRunning`
- From: `Running`
- On: `TaskCreate`(task_id, task_payload)
- Guards:
  - `task_id_unused`
- Emits: `EmitTaskNotice`
- To: `Running`

### `TaskUpdateRunningPending`
- From: `Running`
- On: `TaskUpdate`(task_id, new_status)
- Guards:
  - `target_pending`
  - `task_known`
  - `not_completed`
- Emits: `EmitTaskNotice`
- To: `Running`

### `TaskUpdateRunningInProgress`
- From: `Running`
- On: `TaskUpdate`(task_id, new_status)
- Guards:
  - `target_in_progress`
  - `task_known`
  - `not_completed`
- Emits: `EmitTaskNotice`
- To: `Running`

### `TaskUpdateRunningCompleted`
- From: `Running`
- On: `TaskUpdate`(task_id, new_status)
- Guards:
  - `target_completed`
  - `task_known`
- Emits: `EmitTaskNotice`
- To: `Running`

### `TaskUpdateRunningCancelled`
- From: `Running`
- On: `TaskUpdate`(task_id, new_status)
- Guards:
  - `target_cancelled`
  - `task_known`
  - `not_completed`
- Emits: `EmitTaskNotice`
- To: `Running`

### `ForceCancelRunning`
- From: `Running`
- On: `ForceCancel`()
- Emits: `FlowTerminalized`
- To: `Running`

### `SubscribeAgentEventsRunning`
- From: `Running`
- On: `SubscribeAgentEvents`()
- Guards:
  - `active_members_present`
- To: `Running`

### `SubscribeAgentEventsStopped`
- From: `Stopped`
- On: `SubscribeAgentEvents`()
- Guards:
  - `active_members_present`
- To: `Stopped`

### `SubscribeAgentEventsCompleted`
- From: `Completed`
- On: `SubscribeAgentEvents`()
- Guards:
  - `active_members_present`
- To: `Completed`

### `SubscribeAgentEventsDestroyed`
- From: `Destroyed`
- On: `SubscribeAgentEvents`()
- Guards:
  - `active_members_present`
- To: `Destroyed`

### `SubscribeAllAgentEventsRunning`
- From: `Running`
- On: `SubscribeAllAgentEvents`()
- To: `Running`

### `SubscribeAllAgentEventsStopped`
- From: `Stopped`
- On: `SubscribeAllAgentEvents`()
- To: `Stopped`

### `SubscribeAllAgentEventsCompleted`
- From: `Completed`
- On: `SubscribeAllAgentEvents`()
- To: `Completed`

### `SubscribeAllAgentEventsDestroyed`
- From: `Destroyed`
- On: `SubscribeAllAgentEvents`()
- To: `Destroyed`

### `SubscribeMobEventsRunning`
- From: `Running`
- On: `SubscribeMobEvents`()
- To: `Running`

### `SubscribeMobEventsStopped`
- From: `Stopped`
- On: `SubscribeMobEvents`()
- To: `Stopped`

### `SubscribeMobEventsCompleted`
- From: `Completed`
- On: `SubscribeMobEvents`()
- To: `Completed`

### `SubscribeMobEventsDestroyed`
- From: `Destroyed`
- On: `SubscribeMobEvents`()
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
- Emits: `ExposePendingSpawn`
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
- On: `RunFlow`(run_id, step_ids, ordered_steps, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, escalation_threshold, max_step_retries, max_active_nodes, max_active_frames, max_frame_depth)
- Guards:
  - `coordinator_bound`
  - `run_seed_is_new`
- Emits: `EmitFlowRunNotice`
- To: `Running`

### `CreateRunSeedRunning`
- From: `Running`
- On: `CreateRunSeed`(run_id, step_ids, ordered_steps, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, escalation_threshold, max_step_retries, max_active_nodes, max_active_frames, max_frame_depth)
- Guards:
  - `run_seed_is_new`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `CreateFrameSeedRunning`
- From: `Running`
- On: `CreateFrameSeed`(run_id, frame_id, frame_scope, loop_instance_id, iteration, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, node_step_ids, node_loop_ids, node_status, ready_queue)
- Guards:
  - `frame_seed_is_new`
  - `run_known`
  - `body_frame_has_parent_loop`
  - `frame_seed_status_covers_tracked_nodes`
  - `frame_seed_node_kind_covers_tracked_nodes`
  - `frame_seed_node_kind_keys_are_tracked`
  - `frame_seed_ready_queue_matches_dependency_roots`
  - `frame_seed_step_nodes_have_exact_step_ids`
  - `frame_seed_loop_nodes_have_exact_loop_ids`
  - `frame_seed_step_id_keys_are_tracked_steps`
  - `frame_seed_loop_id_keys_are_tracked_loops`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

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
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `start_run_command`
  - `run_pending`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandDispatchStep`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `dispatch_step_command`
  - `has_step_id`
  - `has_run_step_key`
  - `step_tracked`
  - `dispatched_step_status`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandCompleteStep`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `complete_step_command`
  - `has_step_id`
  - `has_run_step_key`
  - `completed_step_status`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandRecordStepOutput`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `record_step_output_command`
  - `has_step_id`
  - `has_run_step_key`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandConditionPassed`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `condition_passed_command`
  - `has_step_id`
  - `has_run_step_key`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandConditionRejected`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `condition_rejected_command`
  - `has_step_id`
  - `has_run_step_key`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandFailStep`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `fail_step_command`
  - `has_step_id`
  - `has_run_step_key`
  - `failed_step_status`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`, `AppendFailureLedger`
- To: `Running`

### `AuthorizeFlowRunReducerCommandSkipStep`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `skip_step_command`
  - `has_step_id`
  - `has_run_step_key`
  - `skipped_step_status`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandProjectFrameStepStatus`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `project_frame_step_status_command`
  - `has_step_id`
  - `has_run_step_key`
  - `has_frame_id`
  - `has_node_id`
  - `step_tracked`
  - `frame_belongs_to_run`
  - `frame_node_tracked`
  - `frame_node_maps_to_step`
  - `run_step_not_already_terminal_projected`
  - `frame_node_completed_skipped_or_failed`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandCancelStep`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `cancel_step_command`
  - `has_step_id`
  - `has_run_step_key`
  - `step_tracked`
  - `canceled_step_status`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandRegisterTargets`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `register_targets_command`
  - `has_step_id`
  - `has_run_step_key`
  - `has_target_count`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandRecordTargetSuccess`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `record_target_success_command`
  - `has_step_id`
  - `has_run_step_key`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandRecordTargetTerminalFailure`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `record_target_terminal_failure_command`
  - `has_step_id`
  - `has_run_step_key`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandRecordTargetCanceled`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `record_target_canceled_command`
  - `has_step_id`
  - `has_run_step_key`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandRecordTargetFailure`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `record_target_failure_command`
  - `has_step_id`
  - `has_run_step_key`
  - `has_retry_key`
  - `step_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandRegisterReadyFrame`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
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
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
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
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
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
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
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
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
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
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
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
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
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
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
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
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `terminal_completed_command`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandTerminalFailed`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `terminal_failed_command`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowRunReducerCommandTerminalCanceled`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowRunReducerCommand`(run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key)
- Guards:
  - `known_run`
  - `run_running`
  - `terminal_canceled_command`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowFrameReducerCommandAdmitNextReadyNode`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowFrameReducerCommand`(frame_id, command, node_id, frame_node_key, node_status, terminal_status)
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
- On: `AuthorizeFlowFrameReducerCommand`(frame_id, command, node_id, frame_node_key, node_status, terminal_status)
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
- On: `AuthorizeFlowFrameReducerCommand`(frame_id, command, node_id, frame_node_key, node_status, terminal_status)
- Guards:
  - `known_frame`
  - `frame_running`
  - `record_node_output_command`
  - `no_terminal_status`
  - `has_node_id`
  - `has_frame_node_key`
  - `node_tracked`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `AuthorizeFlowFrameReducerCommandFailNode`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowFrameReducerCommand`(frame_id, command, node_id, frame_node_key, node_status, terminal_status)
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
- On: `AuthorizeFlowFrameReducerCommand`(frame_id, command, node_id, frame_node_key, node_status, terminal_status)
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
- On: `AuthorizeFlowFrameReducerCommand`(frame_id, command, node_id, frame_node_key, node_status, terminal_status)
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

### `AuthorizeFlowFrameReducerCommandSealFrame`
- From: `Running`, `Stopped`, `Completed`
- On: `AuthorizeFlowFrameReducerCommand`(frame_id, command, node_id, frame_node_key, node_status, terminal_status)
- Guards:
  - `known_frame`
  - `frame_running`
  - `seal_frame_command`
  - `terminal_frame_status`
  - `all_nodes_terminal`
  - `terminal_class_matches_nodes`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

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
- On: `Retire`(mob_id, agent_runtime_id, agent_identity, releasing, session_id)
- Guards:
  - `active_members_present`
  - `runtime_id_present`
  - `prior_session_binding_present`
  - `releasing_present`
  - `releasing_matches_current`
- Emits: `RequestRuntimeRetire`, `RequestSessionIngressDetachForMobDestroy`, `MemberSessionBindingChanged`
- To: `Running`

### `RetireRunningPreservingBinding`
- From: `Running`
- On: `Retire`(mob_id, agent_runtime_id, agent_identity, releasing, session_id)
- Guards:
  - `active_members_present`
  - `runtime_id_present`
  - `prior_session_binding_present`
  - `releasing_absent`
- Emits: `RequestRuntimeRetire`
- To: `Running`

### `RetireRunningNoBinding`
- From: `Running`
- On: `Retire`(mob_id, agent_runtime_id, agent_identity, releasing, session_id)
- Guards:
  - `active_members_present`
  - `runtime_id_present`
  - `no_prior_session_binding`
  - `releasing_absent`
- Emits: `RequestRuntimeRetire`
- To: `Running`

### `RetireStoppedReleasing`
- From: `Stopped`
- On: `Retire`(mob_id, agent_runtime_id, agent_identity, releasing, session_id)
- Guards:
  - `active_members_present`
  - `runtime_id_present`
  - `prior_session_binding_present`
  - `releasing_present`
  - `releasing_matches_current`
- Emits: `RequestRuntimeRetire`, `RequestSessionIngressDetachForMobDestroy`, `MemberSessionBindingChanged`
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
- On: `Retire`(mob_id, agent_runtime_id, agent_identity, releasing, session_id)
- Guards:
  - `active_members_present`
  - `runtime_id_present`
  - `prior_session_binding_present`
  - `releasing_absent`
- Emits: `RequestRuntimeRetire`
- To: `Stopped`

### `RetireStoppedNoBinding`
- From: `Stopped`
- On: `Retire`(mob_id, agent_runtime_id, agent_identity, releasing, session_id)
- Guards:
  - `active_members_present`
  - `runtime_id_present`
  - `no_prior_session_binding`
  - `releasing_absent`
- Emits: `RequestRuntimeRetire`
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

### `CompleteSpawnRunning`
- From: `Running`, `Stopped`
- On: `CompleteSpawn`(agent_identity)
- Guards:
  - `pending_spawns_present`
  - `pending_identity_present`
- Emits: `EmitMemberLifecycleNotice`
- To: `Running`

### `DestroyFromAny`
- From: `Running`, `Stopped`, `Completed`
- On: `Destroy`()
- Guards:
  - `session_ingress_detaches_closed`
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
- On: `CancelAllWork`(agent_runtime_id, fence_token)
- Guards:
  - `active_members_present`
  - `current_binding_matches`
  - `fence_token_present`
- Emits: `FlowTerminalized`
- To: `Running`

## Coverage
### Code Anchors
- `meerkat-mob/src/runtime/handle.rs` — identity-first public MobMachine handle surface for ensure member, reconcile, and member command routing
- `meerkat-mob/src/runtime/actor.rs` — MobMachine actor authority and command execution for wire, unwire, spawn, ensure member, reconcile, observe runtime, submit work, retire, reset, respawn, complete, mark completed, stop/stopped, resume, task, force cancel, subscribe events, shutdown, destroy, terminalized member, record operator action provenance, flow, run, create frame seed, create loop seed, project frame phase, project loop state, orchestrator, coordinator, cleanup, append failure ledger, escalate supervisor, peer, progress, notices, kickoff resolve started/callback pending/failed/clear, wiring graph, and session binding

### Scenarios
- `spawn-work-terminal` — member spawn, ensure member, reconcile, runtime-ready observation, work submission, and terminal work closure
- `retire-respawn-destroy` — member retires, resets, respawns with a new runtime incarnation, stops/stopped, resumes, shuts down, destroys cleanly, and resets to running when reusable
- `wiring-and-session-binding` — wire and unwire members, enforce known identity for session bindings, expose pending spawn, member session binding changed, and wiring lifecycle notices
- `task-flow-and-run-lifecycle` — task create or update pending/in progress/completed/cancelled, run flow, start flow, create run, create frame seed, create loop seed, project frame phase, project loop state, start run, complete flow, finish run, mark completed, kickoff resolve started or failed, kickoff clear, flow terminalized, and force cancel running work
- `event-subscriptions-and-notices` — subscribe agent, all agent, and mob events; emit member, run, flow, progress, task, terminal, and wiring notices
- `orchestrator-coordinator-cleanup` — initialize, stop, resume, and destroy orchestrator; bind or unbind coordinator; begin and finish cleanup; notify coordinator and escalate supervisor
- `operator-provenance-and-peer-input` — record operator action provenance, trust operation peer, admit peer input, append failure ledger, and surface peer-exposed member inputs
