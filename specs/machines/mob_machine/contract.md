# MobMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::mob_machine`

## State
- Phase enum: `Running | Stopped | Completed | Destroyed`
- `live_runtime_ids`: `Set<AgentRuntimeId>`
- `externally_addressable_runtime_ids`: `Set<AgentRuntimeId>`
- `runtime_fence_tokens`: `Map<AgentRuntimeId, FenceToken>`
- `active_run_count`: `u64`
- `pending_spawn_count`: `u64`
- `coordinator_bound`: `Bool`
- `member_state_markers`: `Map<AgentRuntimeId, MobMemberState>`
- `wiring_edges`: `Set<WiringEdge>`
- `identity_to_runtime`: `Map<AgentIdentity, AgentRuntimeId>`
- `tasks`: `Map<TaskId, MobTask>`
- `in_progress_task_ids`: `Set<TaskId>`
- `completed_task_ids`: `Set<TaskId>`
- `member_session_bindings`: `Map<AgentIdentity, SessionId>`
- `pending_session_ingress_detach_runtime_ids`: `Set<AgentRuntimeId>`
- `topology_epoch`: `u64`

## Inputs
- `RunFlow`
- `CancelFlow`
- `FlowStatus`
- `Spawn`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, external_addressable: Bool, bridge_session_id: SessionId, replacing: Option<SessionId>)
- `Retire`(agent_runtime_id: AgentRuntimeId, agent_identity: AgentIdentity, releasing: Option<SessionId>, session_id: SessionId)
- `Respawn`(agent_runtime_id: AgentRuntimeId)
- `RetireAll`
- `WireMembers`(edge: WiringEdge)
- `UnwireMembers`(edge: WiringEdge)
- `BindMemberSession`(agent_identity: AgentIdentity, session_id: SessionId)
- `RotateMemberSession`(agent_identity: AgentIdentity, old_session_id: SessionId, new_session_id: SessionId)
- `ReleaseMemberSession`(agent_identity: AgentIdentity, session_id: SessionId)
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
- `McpServerStates`
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

## Surface-only Inputs
- `FlowStatus`
- `TaskList`
- `TaskGet`
- `McpServerStates`
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
- `StageSpawn`
- `CompleteSpawn`
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
- `WiringGraphChanged`(epoch: u64)
- `MemberSessionBindingChanged`(epoch: u64, agent_identity: AgentIdentity, old_session_id: Option<SessionId>, new_session_id: Option<SessionId>)
- `EmitWiringLifecycleNotice`(kind: WiringLifecycleKind, edge: WiringEdge)

## Invariants
- `bindings_require_known_identity`

## Transitions
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

### `BindMemberSessionRunning`
- From: `Running`
- On: `BindMemberSession`(agent_identity, session_id)
- Guards:
  - `identity_has_runtime`
  - `no_prior_session_binding`
- Emits: `MemberSessionBindingChanged`
- To: `Running`

### `RotateMemberSessionRunning`
- From: `Running`
- On: `RotateMemberSession`(agent_identity, old_session_id, new_session_id)
- Guards:
  - `identity_has_runtime`
  - `prior_session_binding_present`
  - `old_session_id_matches_current`
- Emits: `MemberSessionBindingChanged`
- To: `Running`

### `ReleaseMemberSessionRunning`
- From: `Running`
- On: `ReleaseMemberSession`(agent_identity, session_id)
- Guards:
  - `prior_session_binding_present`
  - `session_id_matches_current`
- Emits: `MemberSessionBindingChanged`
- To: `Running`

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
- Emits: `RequestRuntimeBinding`, `MemberSessionBindingChanged`, `EmitMemberLifecycleNotice`
- To: `Running`

### `ObserveRuntimeReady`
- From: `Running`
- On: `ObserveRuntimeReady`(agent_runtime_id, fence_token)
- To: `Running`

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
- Emits: `RequestRuntimeRetire`
- To: `Running`

### `ObserveRuntimeRetired`
- From: `Running`
- On: `ObserveRuntimeRetired`(agent_runtime_id, fence_token)
- Guards:
  - `current_binding_matches`
- Emits: `EmitMemberLifecycleNotice`
- To: `Stopped`

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
- On: `CancelFlow`()
- Emits: `FlowTerminalized`
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
- On: `StageSpawn`()
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
- On: `RunFlow`()
- Guards:
  - `coordinator_bound`
- Emits: `EmitFlowRunNotice`
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
- On: `Retire`(agent_runtime_id, agent_identity, releasing, session_id)
- Guards:
  - `active_members_present`
  - `runtime_id_present`
  - `prior_session_binding_present`
  - `releasing_present`
- Emits: `RequestRuntimeRetire`, `MemberSessionBindingChanged`
- To: `Running`

### `RetireRunningPreservingBinding`
- From: `Running`
- On: `Retire`(agent_runtime_id, agent_identity, releasing, session_id)
- Guards:
  - `active_members_present`
  - `runtime_id_present`
  - `prior_session_binding_present`
  - `releasing_absent`
- Emits: `RequestRuntimeRetire`
- To: `Running`

### `RetireRunningNoBinding`
- From: `Running`
- On: `Retire`(agent_runtime_id, agent_identity, releasing, session_id)
- Guards:
  - `active_members_present`
  - `runtime_id_present`
  - `no_prior_session_binding`
  - `releasing_absent`
- Emits: `RequestRuntimeRetire`
- To: `Running`

### `RetireStoppedReleasing`
- From: `Stopped`
- On: `Retire`(agent_runtime_id, agent_identity, releasing, session_id)
- Guards:
  - `active_members_present`
  - `runtime_id_present`
  - `prior_session_binding_present`
  - `releasing_present`
- Emits: `RequestRuntimeRetire`, `MemberSessionBindingChanged`
- To: `Stopped`

### `SessionIngressDetachedForMobDestroyRunning`
- From: `Running`
- On: `SessionIngressDetachedForMobDestroy`(mob_id, agent_runtime_id)
- Guards:
  - `mob_id_present`
  - `pending_detach_present`
- To: `Running`

### `SessionIngressDetachedForMobDestroyStopped`
- From: `Stopped`
- On: `SessionIngressDetachedForMobDestroy`(mob_id, agent_runtime_id)
- Guards:
  - `mob_id_present`
  - `pending_detach_present`
- To: `Stopped`

### `SessionIngressDetachFailedForMobDestroyRunning`
- From: `Running`
- On: `SessionIngressDetachFailedForMobDestroy`(mob_id, agent_runtime_id, reason)
- Guards:
  - `mob_id_present`
  - `reason_present`
  - `pending_detach_present`
- To: `Running`

### `SessionIngressDetachFailedForMobDestroyStopped`
- From: `Stopped`
- On: `SessionIngressDetachFailedForMobDestroy`(mob_id, agent_runtime_id, reason)
- Guards:
  - `mob_id_present`
  - `reason_present`
  - `pending_detach_present`
- To: `Stopped`

### `RetireStoppedPreservingBinding`
- From: `Stopped`
- On: `Retire`(agent_runtime_id, agent_identity, releasing, session_id)
- Guards:
  - `active_members_present`
  - `runtime_id_present`
  - `prior_session_binding_present`
  - `releasing_absent`
- Emits: `RequestRuntimeRetire`
- To: `Stopped`

### `RetireStoppedNoBinding`
- From: `Stopped`
- On: `Retire`(agent_runtime_id, agent_identity, releasing, session_id)
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
- On: `CompleteSpawn`()
- Guards:
  - `pending_spawns_present`
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
- Emits: `FlowTerminalized`
- To: `Running`

## Coverage
### Code Anchors
- `meerkat-mob/src/runtime/handle.rs` — identity-first public MobMachine handle surface
- `meerkat-mob/src/runtime/actor.rs` — MobMachine actor authority and command execution

### Scenarios
- `spawn-work-terminal` — member spawn, runtime-ready observation, work submission, and terminal work closure
- `retire-respawn-destroy` — member retires, respawns with a new runtime incarnation, and destroys cleanly
