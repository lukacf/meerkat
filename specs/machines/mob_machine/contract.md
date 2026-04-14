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

## Inputs
- `Start`
- `SpawnMember`(agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation)
- `ObserveRuntimeReady`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken)
- `SubmitWork`(agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId)
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
- `KickoffBarrierSnapshot`
- `SetSpawnPolicy`
- `Shutdown`
- `ForceCancel`
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

## Transitions
### `Start`
- From: `Creating`, `Stopped`
- On: `Start`()
- To: `Running`

### `SpawnMember`
- From: `Creating`, `Running`, `Stopped`
- On: `SpawnMember`(agent_identity, agent_runtime_id, fence_token, generation)
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
- From: `Running`, `Stopped`
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

## Coverage
### Code Anchors
- `meerkat-mob/src/runtime/handle.rs` — identity-first public MobMachine handle surface
- `meerkat-mob/src/runtime/actor.rs` — MobMachine actor authority and command execution

### Scenarios
- `spawn-work-terminal` — member spawn, runtime-ready observation, work submission, and terminal work closure
- `retire-respawn-destroy` — member retires, respawns with a new runtime incarnation, and destroys cleanly
