# Runtime/Schema Parity Checklist

This checklist tracks the canonical runtime/schema parity contract for the
two-kernel machine schemas.

It is split into three classes:
- **mutating commands**: must be modeled by canonical schema transitions
- **coordination / lock-bearing queries**: observational on paper, but they can
  participate in scheduling, lock ownership, wait semantics, or other behavior
  that can interfere with authoritative operations; they must be explicitly
  modeled and classified
- **pure observation queries**: observational projections only; they must be
  explicitly accounted for, but they are excluded from the mutating-command
  parity bar

## MeerkatMachine

Absorbed domains included in the merged kernel:
- session lifecycle
- control plane
- ingress
- turn/run
- drain/comms
- tool visibility / tool surface
- peer directory reachability

### Mutating runtime commands
- [x] `RegisterSession`
- [x] `UnregisterSession`
- [x] `EnsureSessionWithExecutor`
- [x] `SetSilentIntents`
- [x] `InterruptCurrentRun`
- [x] `CancelAfterBoundary`
- [x] `StopRuntimeExecutor`
- [x] `PrepareBindings`
- [x] `PublishCommittedVisibleSet`
- [x] `SetPeerIngressContext`
- [x] `NotifyDrainExited`
- [x] `AbortAll`
- [x] `Abort`
- [x] `Ingest`
- [x] `PublishEvent`
- [x] `Retire`
- [x] `Recycle`
- [x] `Reset`
- [x] `Recover`
- [x] `Destroy`
- [x] `AcceptWithCompletion`
- [x] `AcceptWithoutWake`
- [x] `Prepare`
- [x] `Commit`
- [x] `Fail`

### Coordination / lock-bearing queries
- [x] `OpsLifecycleRegistry`
- [x] `Wait`

### Pure observation queries
- [x] `ContainsSession`
- [x] `SessionHasExecutor`
- [x] `SessionHasComms`
- [x] `InputState`
- [x] `ListActiveInputs`
- [x] `RuntimeState`
- [x] `LoadBoundaryReceipt`

## MobMachine

Absorbed domains included in the merged kernel:
- lifecycle/runtime bridge
- work ledger
- topology/wiring
- flow/frame/loop execution
- task board
- event/subscription surface

### Mutating runtime commands
- [x] `RunFlow`
- [x] `CancelFlow`
- [x] `Spawn`
- [x] `Retire`
- [x] `Respawn`
- [x] `RetireAll`
- [x] `Wire`
- [x] `Unwire`
- [x] `ExternalTurn`
- [x] `InternalTurn`
- [x] `SubmitWork`
- [x] `CancelWork`
- [x] `CancelAllWork`
- [x] `Stop`
- [x] `Resume`
- [x] `Complete`
- [x] `Reset`
- [x] `Destroy`
- [x] `TaskCreate`
- [x] `TaskUpdate`
- [x] `SubscribeAgentEvents`
- [x] `SubscribeAllAgentEvents`
- [x] `SubscribeMobEvents`
- [x] `RecordOperatorActionProvenance`
- [x] `SetSpawnPolicy`
- [x] `Shutdown`
- [x] `ForceCancel`

### Coordination / lock-bearing queries
- [x] `PollEvents`
- [x] `ReplayAllEvents`
- [x] `KickoffBarrierSnapshot`

### Pure observation queries
- [x] `FlowStatus`
- [x] `TaskList`
- [x] `TaskGet`
- [x] `McpServerStates`
- [x] `RosterSnapshot`
- [x] `ListMembers`
- [x] `ListMembersIncludingRetiring`
- [x] `ListAllMembers`
- [x] `MemberStatus`
- [x] `GetMember`

### Test-only diagnostics intentionally excluded from canonical schema publication
- [ ] `FlowTrackerCounts`
- [ ] `OrchestratorSnapshot`
