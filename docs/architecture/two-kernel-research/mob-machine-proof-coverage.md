# MobMachine Proof Coverage

Status: frozen target-state proof coverage handoff

This note closes the last silent-proof gap for `MobMachine`: every obligation in
`mob-machine-proof-obligations.md` must point to an executable home in the
target scaffold or to an explicit refinement delta outside this frozen target
package.

Use together with:

- `mob-machine-proof-obligations.md`
- `mob-machine-flow-family-coverage.md`
- `mob-machine-transition-catalog.md`
- `mob-machine-derived-predicates.md`
- `mob-machine-fairness-assumptions.md`
- `tla/MobMachineTarget.tla`

## Safety coverage

### Identity / authority

- at most one active `FenceToken` per identity:
  modeled directly in `identity.fence_token`; later theorem over
  `ProvisionIncarnationFresh`, `ProvisionIncarnationRecover`, and `ResetMember`
- respawn preserves generation but advances authority:
  `ProvisionIncarnationRecover`
- reset advances generation and authority together:
  `ResetMember`
- destroyed members have no live binding:
  `DestroyedIdentityInvariant`

### Roster / topology

- roster presence implies known identity:
  `TypeOK`
- coordinator is always rostered, active, and runtime-bound:
  `CoordinatorRosterInvariant`, `CoordinatorLivenessInvariant`
- topology edges reference only rostered members:
  `TypeOK`, `TopologySymmetricInvariant`
- external peer specs reference only active, runtime-bound roster members:
  `ExternalSpecInvariant`, `ExternalSpecLivenessInvariant`
- topology revision is monotonic:
  target transition discipline in `BindCoordinator`, `AddTopologyEdge`,
  `RemoveTopologyEdge`, `PublishExternalSpec`, `ClearExternalSpec`

### Provisioning / kickoff

- kickoff pending implies roster presence:
  `KickoffProvisioningInvariant`
- kickoff pending implies active authority, active member status, and live
  runtime binding:
  `KickoffBarrierInvariant`
- kickoff pending implies autonomous runtime mode:
  `KickoffBarrierInvariant`
- kickoff pending never overlaps pending spawn or restore failure:
  `KickoffBarrierInvariant`
- pending spawn identities are always in `Provisioning` authority state:
  `PendingSpawnConsistencyInvariant`
- pending spawn identities always have a pending runtime binding:
  `PendingSpawnConsistencyInvariant`, `PendingRuntimeInvariant`
- pending runtime-id presence iff pending-spawn presence:
  `PendingRuntimeInvariant`
- rostered pending-spawn identities are only `Pending` or `Broken` and not
  runtime-live:
  `PendingSpawnConsistencyInvariant`
- restore-failure records reference known identities:
  `TypeOK`, `RestoreFailureInvariant`
- restore-failure reason presence iff restore-failure presence:
  `RestoreReasonInvariant`
- identity and recovery checkpoint versions stay aligned:
  `CheckpointVersionInvariant`

### Top-level mob lifecycle

- stopped mob phase requires `active_run_count == 0`:
  `StopMob`, `LifecycleActiveRunCountInvariant`
- completed mob phase requires `active_run_count == 0`:
  `CompleteMob`, `LifecycleActiveRunCountInvariant`
- stop/completion require kickoff pending to be empty:
  `StopMob`, `CompleteMob`
- destroyed mob phase requires empty roster:
  `DestroyMob`, `DestroyedMobInvariant`
- destroyed mob phase requires empty pending spawn:
  `DestroyMob`, `DestroyedMobInvariant`
- destroyed mob phase requires no live work:
  `DestroyMob`, `DestroyedMobInvariant`
- destroyed mob phase is terminal for the machine instance:
  `Next`, `DestroyMob`
- stopped mob phase admits only completion, destroy, or stutter:
  `Next`, `StopMob`
- completed mob phase admits only destroy or stutter:
  `Next`, `CompleteMob`

### Work ledger

- work targets only known identities:
  `WorkTargetInvariant`
- new work only targets dispatch-capable member/runtime bindings:
  `SubmitWork`
- direct `SubmitWork` never implicitly materializes a flow-step dispatch:
  `SubmitWork`
- work references at most one `(run, step)` pair:
  target state schema plus `WorkTargetInvariant`
- live run-bound work only targets machine-visible steps:
  `WorkTargetInvariant`
- terminal step states never retain live bound work:
  `WorkStepStateInvariant`
- terminal run states never retain live bound work:
  `TerminalRunWorkInvariant`
- terminal run states never retain non-terminal step status:
  `TerminalRunStepInvariant`, terminal run transitions
- terminal work has terminal status:
  `TerminalWorkInvariant`
- rejected work is never later accepted or running:
  `TerminalWorkInvariant`, terminal-only work transitions

### Flows

- tracked runs exist in the durable run set:
  `TrackedRunSubsetInvariant`, `TrackedRunFlowBindingInvariant`
- terminal run iff completion presence:
  `RunCompletionTimestampInvariant`
- no duplicate ordered steps:
  `OrderedStepsDistinctInvariant`
- every ordered step has dependency / mode / condition / branch /
  collection-policy / threshold entries:
  total step maps in the target state schema
- every ordered step has an explicit dispatch-mode entry seeded at run start:
  total step maps in the target state schema, `StartFlowRun`,
  `DispatchModeKnownInvariant`
- dispatched steps have explicit dispatch width:
  `DispatchStep`, `DispatchWidthKnownInvariant`
- one-to-one dispatched steps have width `1`:
  `OneToOneDispatchWidthInvariant`
- quorum-dispatched steps have width at least their threshold:
  `QuorumDispatchWidthInvariant`
- dependency targets are known ordered steps:
  `DependencyKnownInvariant`
- no self-dependencies:
  `NoSelfDependencyInvariant`
- no duplicate dependencies:
  modeled by set-valued `step_dependencies`
- `DependencyMode::Any` requires branch-backed dependencies:
  `AnyDependencyInvariant`, `AnyDependencyBranchCoverageInvariant`
- `Any` joins enter explicit ready/pending state before dispatch:
  `ResolveAnyJoin`, `PendingStepReadyInvariant`
- `Pending` join-ready steps carry no bound work:
  `PendingStepHasNoBoundWorkInvariant`
- dispatched steps only arise from dependency-satisfied states:
  `DispatchedStepReadyInvariant`
- dispatched steps always retain live bound work:
  `DispatchedStepHasBoundWorkInvariant`
- bound work count equals stored dispatch width once work is bound:
  `DispatchedStepWidthInvariant`
- quorum-completed steps satisfy stored thresholds:
  `QuorumThresholdInvariant`, `QuorumCompletedObservedInvariant`
- no-dispatch-capacity failure only fires when ready work exists but capacity
  does not:
  `FailRunNoDispatchCapacity`, `HasDispatchReadyStep`,
  `DispatchCapacityAvailable`
- branch-labelled steps also have condition presence:
  `BranchConditionInvariant`
- branch groups agree on dependency sets:
  `BranchGroupDependencyInvariant`
- aggregate shape class is total and mutually exclusive for every ordered step:
  `AggregateShapePartitionInvariant`
- frame / loop structure is only live on loop-capable flow archetypes:
  `StructuredFlowInvariant`
- frame / loop transitions do not churn while live bound work exists:
  `OpenFrame`, `CloseFrame`, `OpenLoop`, `AdvanceLoopIteration`, `CloseLoop`
- loop iteration count implies loop and frame structure:
  `LoopFrameInvariant`
- loop structure implies frame structure:
  `LoopFrameInvariant`, `LoopBodyInvariant`
- structured runs require modern schema:
  `StructuredSchemaInvariant`
- `consecutive_failure_count <= failure_count`:
  `FailureCounterInvariant`
- running runs that lose dispatch capacity while undispatched work remains fail
  explicitly:
  `FailRunNoDispatchCapacity`, `HasDispatchReadyStep`,
  `DispatchCapacityAvailable`

### Tasks / history

- task bindings reference known identities or runs:
  `TaskBindingInvariant`
- live task bindings reference only non-terminal runs:
  `LiveTaskRunBindingInvariant`
- terminal run transitions clear live task bindings:
  `CompleteRun`, `FailRun`, `CancelRun`, `FailRunNoDispatchCapacity`
- terminal tasks have no live run binding:
  `TerminalTaskBindingInvariant`
- per-identity history sequence never exceeds global sequence:
  `HistoryMonotonicInvariant`
- per-identity history last-sequence equals per-identity event count:
  `HistoryIdentitySequenceInvariant`

## Liveness coverage

### Provision fairness

- pending spawn eventually resolves ready or failed:
  `ProvisionProgress`, `TargetFairness`, `FinalizeSpawn`, `FailProvision`

### Kickoff fairness

- kickoff pending eventually resolves or is cleared by teardown or failure:
  `KickoffProgress`, `TargetFairness`, `ResolveKickoff`, `RetireMember`,
  `ResetMember`, `DestroyMember`, `RecordRestoreFailure`

### Work fairness

- accepted work eventually reaches running or terminal:
  `StartWork`, `WorkProgress`, `TargetFairness`
- running work eventually terminalizes or is fenced:
  `WorkProgress`, `TargetFairness`

### Flow-step fairness

- running flows eventually dispatch, resolve, structurally advance, or
  terminalize:
  `FlowDispatchProgress`, `FlowTerminalProgress`, `FlowStructureProgress`,
  `WorkProgress`, `TargetFairness`
- running flows without dispatch capacity eventually terminalize failed:
  `FailRunNoDispatchCapacity`, `FlowTerminalProgress`, `TargetFairness`

### Recovery fairness

- repairable restore failure eventually clears or is replaced:
  `ClearRestoreFailure`, `AdvanceCheckpointVersion`, `RecoveryFairness`,
  `RecoveryFairSpec`, `RestoreFailureEventuallyClearsProp`

### History / task fairness

- history/task persistence eventually completes:
  `HistoryTaskProgress`, `TargetFairness`
- live task lifecycle eventually closes under focused close fairness:
  `CloseTask`, `TaskFairness`, `TaskFairSpec`,
  `LiveTaskEventuallyClosesProp`

## Derived vocabulary coverage

Every predicate named in `mob-machine-derived-predicates.md` is defined in
`tla/MobMachineTarget.tla`.

That means proof work uses the documented vocabulary directly instead of
quietly introducing a second, larger derived language during proof writing.

## Review rule

If an obligation cannot be pointed to here as:

1. an invariant,
2. a canonical transition,
3. a named fairness bundle, or
4. an explicit refinement delta outside this frozen target package,

then the target machine is not honestly frozen yet.
