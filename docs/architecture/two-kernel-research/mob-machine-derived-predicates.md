# MobMachine Derived Predicates

Status: frozen target-state derived vocabulary

This note fixes the named derived predicates that the target-state proof work
is allowed to use.

Every predicate listed here is now defined directly in
`tla/MobMachineTarget.tla`; proof work must not invent a second,
undocumented derived vocabulary.

## Identity predicates

- `KnownIdentity(id)`
- `HasRuntime(id)`
- `HasAuthority(id)`
- `IsProvisioning(id)`
- `IsRetiring(id)`
- `HasBinding(id)`

## Roster predicates

- `Rostered(id)`
- `ActiveMember(id)`
- `AutonomousMember(id)`
- `BrokenMember(id)`
- `KickoffPending(id)`
- `DispatchCapable(id)`
- `DispatchCapacityAvailable`

## Topology predicates

- `CoordinatorBound`
- `TopologicallyLinked(a, b)`
- `HasExternalSpec(id)`

## Work predicates

- `KnownWork(w)`
- `LiveWork(w)`
- `TerminalWork(w)`
- `WorkTargetsRun(w, r)`
- `BoundLiveWork(r, step)`
- `BoundLiveWorkForRun(r)`

## Flow predicates

- `KnownRun(r)`
- `TrackedRun(r)`
- `RunningRun(r)`
- `TerminalRun(r)`
- `AllDependenciesSatisfied(r, step)`
- `AnyDependencySatisfied(r, step)`
- `OneToOneStep(r, step)`
- `FanInStep(r, step)`
- `FanOutStep(r, step)`
- `ReadyDirectDispatchStep(r, step)`
- `ReadyResolvedStep(r, step)`
- `QuorumReady(r, step)`
- `HasDispatchReadyStep(r)`
- `StructuredRun(r)`
- `LoopAwareRun(r)`
- `ReadyStep(r, step)`
- `PendingStepHasNoBoundWork(r, step)`
- `DispatchedStepHasBoundWork(r, step)`
- `BranchMember(r, step)`
- `BranchJoinStep(r, step)`
- `CollectionQuorumStep(r, step)`
- `AggregateScalarStep(r, step)`
- `AggregateArrayStep(r, step)`
- `AggregateObjectStep(r, step)`

## Recovery / history predicates

- `HasRestoreFailure(id)`
- `HasContinuityBinding(id)`
- `HasHistory(id)`
- `HasTask(t)`
- `LiveTaskBinding(t)`
- `HistoryIdentitySequence(id)`
