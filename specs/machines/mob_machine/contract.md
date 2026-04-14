# MobMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-mob` / `generated::mob_machine`

## State
- Phase enum: `Creating | Running | Stopped | Completed | Destroyed`
- `active_identity`: `Option<AgentIdentity>`
- `active_runtime_id`: `Option<AgentRuntimeId>`
- `active_fence_token`: `Option<FenceToken>`
- `current_generation`: `Option<Generation>`
- `inflight_work_id`: `Option<WorkId>`
- `active_member_count`: `u32`
- `active_run_count`: `u32`
- `pending_spawn_count`: `u32`
- `retiring_member_count`: `u32`
- `wiring_edge_count`: `u32`
- `task_count`: `u32`
- `event_subscription_count`: `u32`
- `active_frame_count`: `u32`
- `active_loop_count`: `u32`
- `coordinator_bound`: `Bool`
- `kickoff_pending`: `Bool`

## Inputs
- `Spawn`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `SubmitWork`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId)
- `RunFlow`
- `CancelFlow`
- `FlowStatus`
- `Retire`(agent_runtime_id: AgentRuntimeId)
- `Respawn`(agent_runtime_id: AgentRuntimeId)
- `RetireAll`
- `Wire`
- `Unwire`
- `ExternalTurn`
- `InternalTurn`
- `CancelWork`(work_id: WorkId)
- `CancelAllWork`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `Stop`
- `Resume`
- `Complete`
- `Reset`
- `Destroy`
- `TaskCreate`
- `TaskUpdate`
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

## Signals
- `Start`
- `ObserveRuntimeReady`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `ObserveWorkCompleted`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId)
- `ObserveWorkFailed`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId)
- `ObserveWorkCancelled`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId)
- `RetireMember`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `ObserveRuntimeRetired`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `ResetMember`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `RespawnMember`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `DestroyMob`
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
- `RuntimeWorkAdmitted`
- `KickoffStarted`
- `KickoffCallbackPending`
- `KickoffFailed`
- `KickoffCancelled`
- `KickoffForceCancelled`
- `RuntimeRunSubmitted`
- `RuntimeRunCompleted`
- `RuntimeRunFailed`
- `RuntimeRunCancelled`
- `RuntimeStopRequested`
- `CreateRun`
- `DispatchStep`
- `CompleteStep`
- `RecordStepOutput`
- `ConditionPassed`
- `ConditionRejected`
- `FailStep`
- `SkipStep`
- `ProjectFrameStepStatus`
- `CancelStep`
- `RegisterTargets`
- `RecordTargetSuccess`
- `RecordTargetTerminalFailure`
- `RecordTargetCanceled`
- `RecordTargetFailure`
- `RegisterReadyFrame`
- `RegisterPendingBodyFrame`
- `NodeExecutionReleased`
- `FrameTerminated`
- `TerminalizeCompleted`
- `TerminalizeFailed`
- `TerminalizeCanceled`
- `StartRootFrame`
- `StartBodyFrame`
- `CompleteNode`
- `RecordNodeOutput`
- `FailNode`
- `SkipNode`
- `CancelNode`
- `StartLoop`
- `BodyFrameStarted`
- `BodyFrameCompleted`
- `BodyFrameFailed`
- `BodyFrameCanceled`
- `UntilConditionMet`
- `UntilConditionFailed`
- `CancelLoop`

## Effects
- `RequestRuntimeBinding`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `SubmitMemberWork`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId)
- `RequestRuntimeRetire`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `RequestRuntimeDestroy`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `EmitMemberLifecycleNotice`(agent_identity: AgentIdentity, kind: String)
- `EmitRunLifecycleNotice`
- `EmitFlowRunNotice`
- `EmitStepNotice`
- `AppendFailureLedger`
- `PersistStepOutput`
- `AdmitStepWork`
- `FlowTerminalized`
- `EscalateSupervisor`
- `ProjectTargetSuccess`
- `ProjectTargetFailure`
- `ProjectTargetCanceled`
- `GrantNodeSlot`
- `GrantBodyFrameStart`
- `NotifyCoordinator`
- `ExposePendingSpawn`
- `AdmitKickoffTurn`
- `EmitMemberTerminalNotice`
- `AdmitPeerInput`
- `EmitProgressNote`
- `EmitTaskNotice`
- `ReadyFrontierChanged`
- `StartLoopNode`
- `NodeExecutionReleased`
- `RootFrameCompleted`
- `RootFrameFailed`
- `RootFrameCanceled`
- `BodyFrameCompleted`
- `BodyFrameFailed`
- `BodyFrameCanceled`
- `RequestBodyFrameStart`
- `EvaluateUntilCondition`
- `LoopCompleted`
- `LoopExhausted`
- `LoopFailed`
- `LoopCanceled`

## Invariants
- `active_work_requires_runtime`
- `destroyed_has_no_active_runtime`
- `active_runtime_has_identity`
- `active_frames_require_runs`
- `active_loops_require_frames`
- `retiring_members_do_not_exceed_active_members`
- `kickoff_pending_requires_members`

## Transitions
### `Start`
- From: `Creating`, `Stopped`
- On: `Start`()
- To: `Running`

### `Spawn`
- From: `Creating`, `Running`
- On: `Spawn`(agent_identity, agent_runtime_id, fence_token, generation)
- Emits: `RequestRuntimeBinding`, `EmitMemberLifecycleNotice`
- To: `Running`

### `ObserveRuntimeReady`
- From: `Running`
- On: `ObserveRuntimeReady`(agent_runtime_id, fence_token)
- To: `Running`

### `SubmitWork`
- From: `Running`
- On: `SubmitWork`(agent_runtime_id, fence_token, work_id)
- Guards:
  - `runtime_is_bound`
- Emits: `SubmitMemberWork`
- To: `Running`

### `ObserveWorkCompleted`
- From: `Running`
- On: `ObserveWorkCompleted`(agent_runtime_id, fence_token, work_id)
- To: `Running`

### `ObserveWorkFailed`
- From: `Running`
- On: `ObserveWorkFailed`(agent_runtime_id, fence_token, work_id)
- To: `Running`

### `ObserveWorkCancelled`
- From: `Running`
- On: `ObserveWorkCancelled`(agent_runtime_id, fence_token, work_id)
- To: `Running`

### `RetireMember`
- From: `Running`
- On: `RetireMember`(agent_runtime_id, fence_token)
- Emits: `RequestRuntimeRetire`
- To: `Running`

### `ObserveRuntimeRetired`
- From: `Running`
- On: `ObserveRuntimeRetired`(agent_runtime_id, fence_token)
- Emits: `EmitMemberLifecycleNotice`
- To: `Stopped`

### `ResetMember`
- From: `Running`, `Stopped`
- On: `ResetMember`(agent_identity, agent_runtime_id, fence_token, generation)
- Emits: `RequestRuntimeBinding`, `EmitMemberLifecycleNotice`
- To: `Running`

### `RespawnMember`
- From: `Creating`, `Running`
- On: `RespawnMember`(agent_identity, agent_runtime_id, fence_token, generation)
- Emits: `RequestRuntimeBinding`, `EmitMemberLifecycleNotice`
- To: `Running`

### `MarkCompleted`
- From: `Running`, `Stopped`
- On: `MarkCompleted`()
- Guards:
  - `no_inflight_work`
- Emits: `EmitMemberLifecycleNotice`
- To: `Completed`

### `DestroyMob`
- From: `Creating`, `Running`, `Stopped`, `Completed`
- On: `DestroyMob`()
- Emits: `RequestRuntimeDestroy`
- To: `Destroyed`

### `ObserveRuntimeDestroyed`
- From: `Running`, `Stopped`, `Completed`, `Destroyed`
- On: `ObserveRuntimeDestroyed`(agent_runtime_id, fence_token)
- Emits: `EmitMemberLifecycleNotice`
- To: `Destroyed`

### `FlowStatusCreating`
- From: `Creating`
- On: `FlowStatus`()
- To: `Creating`

### `FlowStatusRunning`
- From: `Running`
- On: `FlowStatus`()
- To: `Running`

### `FlowStatusStopped`
- From: `Stopped`
- On: `FlowStatus`()
- To: `Stopped`

### `FlowStatusCompleted`
- From: `Completed`
- On: `FlowStatus`()
- To: `Completed`

### `FlowStatusDestroyed`
- From: `Destroyed`
- On: `FlowStatus`()
- To: `Destroyed`

### `McpServerStatesCreating`
- From: `Creating`
- On: `McpServerStates`()
- To: `Creating`

### `McpServerStatesRunning`
- From: `Running`
- On: `McpServerStates`()
- To: `Running`

### `McpServerStatesStopped`
- From: `Stopped`
- On: `McpServerStates`()
- To: `Stopped`

### `McpServerStatesCompleted`
- From: `Completed`
- On: `McpServerStates`()
- To: `Completed`

### `McpServerStatesDestroyed`
- From: `Destroyed`
- On: `McpServerStates`()
- To: `Destroyed`

### `RosterSnapshotCreating`
- From: `Creating`
- On: `RosterSnapshot`()
- To: `Creating`

### `RosterSnapshotRunning`
- From: `Running`
- On: `RosterSnapshot`()
- To: `Running`

### `RosterSnapshotStopped`
- From: `Stopped`
- On: `RosterSnapshot`()
- To: `Stopped`

### `RosterSnapshotCompleted`
- From: `Completed`
- On: `RosterSnapshot`()
- To: `Completed`

### `RosterSnapshotDestroyed`
- From: `Destroyed`
- On: `RosterSnapshot`()
- To: `Destroyed`

### `ListMembersCreating`
- From: `Creating`
- On: `ListMembers`()
- To: `Creating`

### `ListMembersRunning`
- From: `Running`
- On: `ListMembers`()
- To: `Running`

### `ListMembersStopped`
- From: `Stopped`
- On: `ListMembers`()
- To: `Stopped`

### `ListMembersCompleted`
- From: `Completed`
- On: `ListMembers`()
- To: `Completed`

### `ListMembersDestroyed`
- From: `Destroyed`
- On: `ListMembers`()
- To: `Destroyed`

### `ListMembersIncludingRetiringCreating`
- From: `Creating`
- On: `ListMembersIncludingRetiring`()
- To: `Creating`

### `ListMembersIncludingRetiringRunning`
- From: `Running`
- On: `ListMembersIncludingRetiring`()
- To: `Running`

### `ListMembersIncludingRetiringStopped`
- From: `Stopped`
- On: `ListMembersIncludingRetiring`()
- To: `Stopped`

### `ListMembersIncludingRetiringCompleted`
- From: `Completed`
- On: `ListMembersIncludingRetiring`()
- To: `Completed`

### `ListMembersIncludingRetiringDestroyed`
- From: `Destroyed`
- On: `ListMembersIncludingRetiring`()
- To: `Destroyed`

### `ListAllMembersCreating`
- From: `Creating`
- On: `ListAllMembers`()
- To: `Creating`

### `ListAllMembersRunning`
- From: `Running`
- On: `ListAllMembers`()
- To: `Running`

### `ListAllMembersStopped`
- From: `Stopped`
- On: `ListAllMembers`()
- To: `Stopped`

### `ListAllMembersCompleted`
- From: `Completed`
- On: `ListAllMembers`()
- To: `Completed`

### `ListAllMembersDestroyed`
- From: `Destroyed`
- On: `ListAllMembers`()
- To: `Destroyed`

### `MemberStatusCreating`
- From: `Creating`
- On: `MemberStatus`()
- To: `Creating`

### `MemberStatusRunning`
- From: `Running`
- On: `MemberStatus`()
- To: `Running`

### `MemberStatusStopped`
- From: `Stopped`
- On: `MemberStatus`()
- To: `Stopped`

### `MemberStatusCompleted`
- From: `Completed`
- On: `MemberStatus`()
- To: `Completed`

### `MemberStatusDestroyed`
- From: `Destroyed`
- On: `MemberStatus`()
- To: `Destroyed`

### `TaskListCreating`
- From: `Creating`
- On: `TaskList`()
- To: `Creating`

### `TaskListRunning`
- From: `Running`
- On: `TaskList`()
- To: `Running`

### `TaskListStopped`
- From: `Stopped`
- On: `TaskList`()
- To: `Stopped`

### `TaskListCompleted`
- From: `Completed`
- On: `TaskList`()
- To: `Completed`

### `TaskListDestroyed`
- From: `Destroyed`
- On: `TaskList`()
- To: `Destroyed`

### `TaskGetCreating`
- From: `Creating`
- On: `TaskGet`()
- To: `Creating`

### `TaskGetRunning`
- From: `Running`
- On: `TaskGet`()
- To: `Running`

### `TaskGetStopped`
- From: `Stopped`
- On: `TaskGet`()
- To: `Stopped`

### `TaskGetCompleted`
- From: `Completed`
- On: `TaskGet`()
- To: `Completed`

### `TaskGetDestroyed`
- From: `Destroyed`
- On: `TaskGet`()
- To: `Destroyed`

### `PollEventsCreating`
- From: `Creating`
- On: `PollEvents`()
- To: `Creating`

### `PollEventsRunning`
- From: `Running`
- On: `PollEvents`()
- To: `Running`

### `PollEventsStopped`
- From: `Stopped`
- On: `PollEvents`()
- To: `Stopped`

### `PollEventsCompleted`
- From: `Completed`
- On: `PollEvents`()
- To: `Completed`

### `PollEventsDestroyed`
- From: `Destroyed`
- On: `PollEvents`()
- To: `Destroyed`

### `ReplayAllEventsCreating`
- From: `Creating`
- On: `ReplayAllEvents`()
- To: `Creating`

### `ReplayAllEventsRunning`
- From: `Running`
- On: `ReplayAllEvents`()
- To: `Running`

### `ReplayAllEventsStopped`
- From: `Stopped`
- On: `ReplayAllEvents`()
- To: `Stopped`

### `ReplayAllEventsCompleted`
- From: `Completed`
- On: `ReplayAllEvents`()
- To: `Completed`

### `ReplayAllEventsDestroyed`
- From: `Destroyed`
- On: `ReplayAllEvents`()
- To: `Destroyed`

### `RecordOperatorActionProvenanceCreating`
- From: `Creating`
- On: `RecordOperatorActionProvenance`()
- To: `Creating`

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

### `GetMemberCreating`
- From: `Creating`
- On: `GetMember`()
- To: `Creating`

### `GetMemberRunning`
- From: `Running`
- On: `GetMember`()
- To: `Running`

### `GetMemberStopped`
- From: `Stopped`
- On: `GetMember`()
- To: `Stopped`

### `GetMemberCompleted`
- From: `Completed`
- On: `GetMember`()
- To: `Completed`

### `GetMemberDestroyed`
- From: `Destroyed`
- On: `GetMember`()
- To: `Destroyed`

### `SetSpawnPolicyCreating`
- From: `Creating`
- On: `SetSpawnPolicy`()
- To: `Creating`

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

### `WireCreating`
- From: `Creating`
- On: `Wire`()
- Emits: `NotifyCoordinator`
- To: `Creating`

### `WireRunning`
- From: `Running`
- On: `Wire`()
- Emits: `NotifyCoordinator`
- To: `Running`

### `ExternalTurnCreating`
- From: `Creating`
- On: `ExternalTurn`()
- Emits: `EmitProgressNote`
- To: `Creating`

### `ExternalTurnRunning`
- From: `Running`
- On: `ExternalTurn`()
- Emits: `EmitProgressNote`
- To: `Running`

### `InternalTurnCreating`
- From: `Creating`
- On: `InternalTurn`()
- Emits: `EmitProgressNote`
- To: `Creating`

### `InternalTurnRunning`
- From: `Running`
- On: `InternalTurn`()
- Emits: `EmitProgressNote`
- To: `Running`

### `TaskCreateCreating`
- From: `Creating`
- On: `TaskCreate`()
- Emits: `EmitTaskNotice`
- To: `Creating`

### `TaskCreateRunning`
- From: `Running`
- On: `TaskCreate`()
- Emits: `EmitTaskNotice`
- To: `Running`

### `TaskUpdateCreating`
- From: `Creating`
- On: `TaskUpdate`()
- Emits: `EmitTaskNotice`
- To: `Creating`

### `TaskUpdateRunning`
- From: `Running`
- On: `TaskUpdate`()
- Emits: `EmitTaskNotice`
- To: `Running`

### `ForceCancelCreating`
- From: `Creating`
- On: `ForceCancel`()
- Emits: `FlowTerminalized`
- To: `Creating`

### `ForceCancelRunning`
- From: `Running`
- On: `ForceCancel`()
- Emits: `FlowTerminalized`
- To: `Running`

### `SubscribeAgentEventsCreating`
- From: `Creating`
- On: `SubscribeAgentEvents`()
- To: `Creating`

### `SubscribeAgentEventsRunning`
- From: `Running`
- On: `SubscribeAgentEvents`()
- To: `Running`

### `SubscribeAgentEventsStopped`
- From: `Stopped`
- On: `SubscribeAgentEvents`()
- To: `Stopped`

### `SubscribeAgentEventsCompleted`
- From: `Completed`
- On: `SubscribeAgentEvents`()
- To: `Completed`

### `SubscribeAgentEventsDestroyed`
- From: `Destroyed`
- On: `SubscribeAgentEvents`()
- To: `Destroyed`

### `SubscribeAllAgentEventsCreating`
- From: `Creating`
- On: `SubscribeAllAgentEvents`()
- To: `Creating`

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

### `SubscribeMobEventsCreating`
- From: `Creating`
- On: `SubscribeMobEvents`()
- To: `Creating`

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

### `ShutdownCreating`
- From: `Creating`
- On: `Shutdown`()
- Emits: `EmitRunLifecycleNotice`
- To: `Creating`

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

### `ResumeOrchestratorRunning`
- From: `Running`
- On: `ResumeOrchestrator`()
- Emits: `NotifyCoordinator`
- To: `Running`

### `DestroyOrchestratorRunning`
- From: `Running`
- On: `DestroyOrchestrator`()
- Emits: `NotifyCoordinator`
- To: `Running`

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

### `RuntimeWorkAdmittedRunning`
- From: `Running`
- On: `RuntimeWorkAdmitted`()
- Emits: `AdmitStepWork`
- To: `Running`

### `KickoffFailedRunning`
- From: `Running`
- On: `KickoffFailed`()
- Emits: `EmitMemberTerminalNotice`
- To: `Running`

### `KickoffCancelledRunning`
- From: `Running`
- On: `KickoffCancelled`()
- Emits: `EmitMemberTerminalNotice`
- To: `Running`

### `KickoffForceCancelledRunning`
- From: `Running`
- On: `KickoffForceCancelled`()
- Emits: `EmitMemberTerminalNotice`
- To: `Running`

### `RuntimeRunSubmittedRunning`
- From: `Running`
- On: `RuntimeRunSubmitted`()
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `RuntimeRunCompletedRunning`
- From: `Running`
- On: `RuntimeRunCompleted`()
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `RuntimeRunFailedRunning`
- From: `Running`
- On: `RuntimeRunFailed`()
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `RuntimeRunCancelledRunning`
- From: `Running`
- On: `RuntimeRunCancelled`()
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `RuntimeStopRequestedRunning`
- From: `Running`
- On: `RuntimeStopRequested`()
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `DispatchStepRunning`
- From: `Running`
- On: `DispatchStep`()
- Emits: `AdmitStepWork`
- To: `Running`

### `CompleteStepRunning`
- From: `Running`
- On: `CompleteStep`()
- Emits: `EmitStepNotice`
- To: `Running`

### `RecordStepOutputRunning`
- From: `Running`
- On: `RecordStepOutput`()
- Emits: `PersistStepOutput`
- To: `Running`

### `ConditionPassedRunning`
- From: `Running`
- On: `ConditionPassed`()
- Emits: `EmitStepNotice`
- To: `Running`

### `ConditionRejectedRunning`
- From: `Running`
- On: `ConditionRejected`()
- Emits: `EmitStepNotice`
- To: `Running`

### `FailStepRunning`
- From: `Running`
- On: `FailStep`()
- Emits: `EmitStepNotice`
- To: `Running`

### `SkipStepRunning`
- From: `Running`
- On: `SkipStep`()
- Emits: `EmitStepNotice`
- To: `Running`

### `ProjectFrameStepStatusRunning`
- From: `Running`
- On: `ProjectFrameStepStatus`()
- Emits: `EmitStepNotice`
- To: `Running`

### `CancelStepRunning`
- From: `Running`
- On: `CancelStep`()
- Emits: `EmitStepNotice`
- To: `Running`

### `RegisterTargetsRunning`
- From: `Running`
- On: `RegisterTargets`()
- Emits: `NotifyCoordinator`
- To: `Running`

### `RecordTargetSuccessRunning`
- From: `Running`
- On: `RecordTargetSuccess`()
- Emits: `ProjectTargetSuccess`
- To: `Running`

### `RecordTargetTerminalFailureRunning`
- From: `Running`
- On: `RecordTargetTerminalFailure`()
- Emits: `ProjectTargetFailure`
- To: `Running`

### `RecordTargetCanceledRunning`
- From: `Running`
- On: `RecordTargetCanceled`()
- Emits: `ProjectTargetCanceled`
- To: `Running`

### `RecordTargetFailureRunning`
- From: `Running`
- On: `RecordTargetFailure`()
- Emits: `ProjectTargetFailure`
- To: `Running`

### `NodeExecutionReleasedRunning`
- From: `Running`
- On: `NodeExecutionReleased`()
- Emits: `NodeExecutionReleased`
- To: `Running`

### `TerminalizeCompletedRunning`
- From: `Running`
- On: `TerminalizeCompleted`()
- Emits: `RootFrameCompleted`
- To: `Running`

### `TerminalizeFailedRunning`
- From: `Running`
- On: `TerminalizeFailed`()
- Emits: `RootFrameFailed`
- To: `Running`

### `TerminalizeCanceledRunning`
- From: `Running`
- On: `TerminalizeCanceled`()
- Emits: `RootFrameCanceled`
- To: `Running`

### `CompleteNodeRunning`
- From: `Running`
- On: `CompleteNode`()
- Emits: `EmitStepNotice`
- To: `Running`

### `RecordNodeOutputRunning`
- From: `Running`
- On: `RecordNodeOutput`()
- Emits: `PersistStepOutput`
- To: `Running`

### `FailNodeRunning`
- From: `Running`
- On: `FailNode`()
- Emits: `EmitStepNotice`
- To: `Running`

### `SkipNodeRunning`
- From: `Running`
- On: `SkipNode`()
- Emits: `EmitStepNotice`
- To: `Running`

### `CancelNodeRunning`
- From: `Running`
- On: `CancelNode`()
- Emits: `EmitStepNotice`
- To: `Running`

### `UntilConditionMetRunning`
- From: `Running`
- On: `UntilConditionMet`()
- Emits: `EvaluateUntilCondition`
- To: `Running`

### `BeginCleanupRunning`
- From: `Running`
- On: `BeginCleanup`()
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `FinishCleanupRunning`
- From: `Running`
- On: `FinishCleanup`()
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `KickoffStartedRunning`
- From: `Running`
- On: `KickoffStarted`()
- Guards:
  - `active_members_present`
- Emits: `AdmitKickoffTurn`
- To: `Running`

### `KickoffCallbackPendingRunning`
- From: `Running`
- On: `KickoffCallbackPending`()
- Guards:
  - `active_members_present`
- To: `Running`

### `RunFlowRunning`
- From: `Running`
- On: `RunFlow`()
- Guards:
  - `active_members_present`
  - `runtime_is_bound`
- Emits: `EmitFlowRunNotice`
- To: `Running`

### `StartFlowRunning`
- From: `Running`
- On: `StartFlow`()
- Guards:
  - `active_members_present`
  - `runtime_is_bound`
- Emits: `EmitFlowRunNotice`
- To: `Running`

### `CreateRunRunning`
- From: `Running`
- On: `CreateRun`()
- Guards:
  - `active_members_present`
  - `runtime_is_bound`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `StartRunRunning`
- From: `Running`
- On: `StartRun`()
- Guards:
  - `active_members_present`
  - `runtime_is_bound`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `RegisterReadyFrameRunning`
- From: `Running`
- On: `RegisterReadyFrame`()
- Guards:
  - `active_runs_present`
- To: `Running`

### `UnwireCreating`
- From: `Creating`
- On: `Unwire`()
- Guards:
  - `wired_edges_present`
- Emits: `NotifyCoordinator`
- To: `Creating`

### `UnwireRunning`
- From: `Running`
- On: `Unwire`()
- Guards:
  - `wired_edges_present`
- Emits: `NotifyCoordinator`
- To: `Running`

### `RegisterPendingBodyFrameRunning`
- From: `Running`
- On: `RegisterPendingBodyFrame`()
- Guards:
  - `active_runs_present`
- Emits: `RequestBodyFrameStart`
- To: `Running`

### `CompleteFlowRunning`
- From: `Running`
- On: `CompleteFlow`()
- Guards:
  - `active_runs_present`
- Emits: `FlowTerminalized`
- To: `Running`

### `StartRootFrameRunning`
- From: `Running`
- On: `StartRootFrame`()
- Guards:
  - `active_runs_present`
- To: `Running`

### `StartBodyFrameRunning`
- From: `Running`
- On: `StartBodyFrame`()
- Guards:
  - `active_runs_present`
- Emits: `RequestBodyFrameStart`
- To: `Running`

### `FrameTerminatedRunning`
- From: `Running`
- On: `FrameTerminated`()
- Guards:
  - `active_frames_present`
- To: `Running`

### `StartLoopRunning`
- From: `Running`
- On: `StartLoop`()
- Guards:
  - `active_runs_present`
  - `active_frames_present`
- Emits: `StartLoopNode`
- To: `Running`

### `BodyFrameStartedRunning`
- From: `Running`
- On: `BodyFrameStarted`()
- Guards:
  - `active_runs_present`
- Emits: `RequestBodyFrameStart`
- To: `Running`

### `BodyFrameCompletedRunning`
- From: `Running`
- On: `BodyFrameCompleted`()
- Guards:
  - `active_frames_present`
- Emits: `BodyFrameCompleted`
- To: `Running`

### `BodyFrameFailedRunning`
- From: `Running`
- On: `BodyFrameFailed`()
- Guards:
  - `active_frames_present`
- Emits: `BodyFrameFailed`
- To: `Running`

### `BodyFrameCanceledRunning`
- From: `Running`
- On: `BodyFrameCanceled`()
- Guards:
  - `active_frames_present`
- Emits: `BodyFrameCanceled`
- To: `Running`

### `UntilConditionFailedRunning`
- From: `Running`
- On: `UntilConditionFailed`()
- Guards:
  - `active_loops_present`
- Emits: `LoopCompleted`
- To: `Running`

### `CancelLoopRunning`
- From: `Running`
- On: `CancelLoop`()
- Guards:
  - `active_loops_present`
- Emits: `LoopCanceled`
- To: `Running`

### `FinishRunRunning`
- From: `Running`
- On: `FinishRun`()
- Guards:
  - `active_runs_present`
- Emits: `EmitRunLifecycleNotice`
- To: `Running`

### `RetireCreating`
- From: `Creating`
- On: `Retire`(agent_runtime_id)
- Guards:
  - `active_members_present`
  - `unretired_members_present`
- Emits: `RequestRuntimeRetire`
- To: `Creating`

### `RetireRunning`
- From: `Running`
- On: `Retire`(agent_runtime_id)
- Guards:
  - `active_members_present`
  - `unretired_members_present`
- Emits: `RequestRuntimeRetire`
- To: `Running`

### `RetireStopped`
- From: `Stopped`
- On: `Retire`(agent_runtime_id)
- Guards:
  - `active_members_present`
  - `unretired_members_present`
- Emits: `RequestRuntimeRetire`
- To: `Stopped`

### `RetireAllCreating`
- From: `Creating`
- On: `RetireAll`()
- Emits: `EmitMemberLifecycleNotice`
- To: `Creating`

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
- From: `Running`
- On: `CompleteSpawn`()
- Guards:
  - `pending_spawns_present`
- Emits: `EmitMemberLifecycleNotice`
- To: `Running`

### `DestroyFromAny`
- From: `Creating`, `Running`, `Stopped`, `Completed`
- On: `Destroy`()
- Emits: `EmitMemberLifecycleNotice`
- To: `Destroyed`

### `RespawnCreating`
- From: `Creating`
- On: `Respawn`(agent_runtime_id)
- Emits: `ExposePendingSpawn`
- To: `Creating`

### `RespawnRunning`
- From: `Running`
- On: `Respawn`(agent_runtime_id)
- Emits: `ExposePendingSpawn`
- To: `Running`

### `CancelWorkRunning`
- From: `Running`
- On: `CancelWork`(work_id)
- Emits: `FlowTerminalized`
- To: `Running`

### `CancelAllWorkCreating`
- From: `Creating`
- On: `CancelAllWork`()
- Emits: `FlowTerminalized`
- To: `Creating`

### `CancelAllWorkRunning`
- From: `Running`
- On: `CancelAllWork`()
- Emits: `FlowTerminalized`
- To: `Running`

## Coverage
### Code Anchors
- `meerkat-mob/src/runtime/handle.rs` — identity-first public MobMachine handle surface
- `meerkat-mob/src/runtime/actor.rs` — MobMachine actor authority and command execution

### Scenarios
- `spawn-work-terminal` — member spawn, runtime-ready observation, work submission, and terminal work closure
- `retire-respawn-destroy` — member retires, respawns with a new runtime incarnation, and destroys cleanly
