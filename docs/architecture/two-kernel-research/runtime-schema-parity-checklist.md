# Runtime/Schema Parity Checklist

This checklist tracks **state-altering runtime commands** that must be modeled by
the canonical two-kernel machine schemas. Query/read-only surfaces are listed
separately and are intentionally excluded from the mutating-command parity bar.

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

### Read-only/query commands intentionally excluded from the mutating bar
- [ ] `ContainsSession`
- [ ] `SessionHasExecutor`
- [ ] `SessionHasComms`
- [ ] `OpsLifecycleRegistry`
- [ ] `InputState`
- [ ] `ListActiveInputs`
- [ ] `RuntimeState`
- [ ] `LoadBoundaryReceipt`
- [ ] `Wait`

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

### Read-only/query commands intentionally excluded from the mutating bar
- [ ] `FlowStatus`
- [ ] `TaskList`
- [ ] `TaskGet`
- [ ] `McpServerStates`
- [ ] `RosterSnapshot`
- [ ] `ListMembers`
- [ ] `ListMembersIncludingRetiring`
- [ ] `ListAllMembers`
- [ ] `MemberStatus`
- [ ] `PollEvents`
- [ ] `ReplayAllEvents`
- [ ] `GetMember`
- [ ] `KickoffBarrierSnapshot`

### Test-only diagnostics intentionally excluded from canonical schema publication
- [ ] `FlowTrackerCounts`
- [ ] `OrchestratorSnapshot`
