# MobMachine Transition Catalog

Status: frozen target-state transition handoff

This note is the canonical transition catalog for the target-state
`MobMachine`.

Use it together with:

- `mob-machine-freeze.md`
- `mob-machine-state-schema.md`
- `mob-machine-proof-obligations.md`
- `mob-machine-exact-current-freeze.md`

## Transition Form

Every target transition is modeled as:

1. machine input or internally triggered machine action
2. preconditions over machine state
3. state updates over one or more machine regions
4. emitted effects, if any
5. preserved regions, if not updated

No meaningful semantic state change is allowed to occur as an unnamed helper
side path.

## Transition Families

### 1. Identity / authority lifecycle

#### `RegisterIdentity`

- inserts a previously absent `AgentIdentity`
- initializes generation `0`
- allocates initial authority state

#### `ProvisionIncarnationFresh`

- requires a known identity without an active binding
- mints a fresh `AgentRuntimeId`
- issues a `FenceToken`
- inserts pending-spawn lineage

#### `ProvisionIncarnationRecover`

- requires a known identity with continuity to recover
- preserves `Generation`
- preserves `AgentRuntimeId`
- issues a strictly newer `FenceToken`
- inserts pending-spawn lineage

#### `FinalizeSpawn`

- removes the identity from pending-spawn lineage
- materializes roster presence
- binds hidden runtime presence
- autonomous members enter kickoff pending state
- turn-driven members finalize without kickoff pending state

#### `FailProvision`

- removes the identity from pending-spawn lineage
- records a restore or provisioning failure
- clears any partial binding
- clears coordinator or external-spec presence if they referenced that identity

#### `ResolveKickoff`

- removes one autonomous member from kickoff pending state
- preserves roster presence and live runtime binding
- marks the kickoff barrier as satisfied for that member

#### `RetireMember`

- transitions authority into retiring posture
- forbids new work submission for that identity
- preserves already accepted work
- clears kickoff pending state for that identity
- clears coordinator or external-spec presence if they referenced that identity

#### `ResetMember`

- advances `Generation`
- replaces `AgentRuntimeId`
- issues fresh authority
- fences the old runtime immediately
- clears any previous kickoff pending state
- advances identity and recovery checkpoint versions together
- clears coordinator or external-spec presence if they referenced that identity

#### `DestroyMember`

- clears active binding
- removes roster presence
- clears kickoff / pending-spawn presence
- clears coordinator or external-spec presence if they referenced that identity
- leaves durable identity history intact

### 2. Topology

#### `BindCoordinator`

- sets or replaces the coordinator identity
- requires an active, rostered, runtime-bound member
- bumps topology revision

#### `AddTopologyEdge`

- inserts a collaboration edge
- bumps topology revision

#### `RemoveTopologyEdge`

- removes a collaboration edge
- bumps topology revision

#### `PublishExternalSpec`

- marks one active, rostered, runtime-bound member as externally spec-addressable
- bumps topology revision

#### `ClearExternalSpec`

- removes external-spec presence for one member
- bumps topology revision

### 3. Work ledger

#### `SubmitWork`

- allocates a `WorkRef`
- requires a dispatch-capable member target
- binds it to identity and runtime targets
- does not directly materialize flow-step dispatch
- enters `Pending`

#### `AcceptWork`

- transitions `Pending -> Accepted`

#### `StartWork`

- transitions `Accepted -> Running`

#### `RejectWork`

- transitions `Pending -> Rejected`
- records a terminal outcome
- preserves step/run bindings for later flow-side reconciliation

#### `CompleteWork`

- transitions `Running -> Completed`
- records a terminal outcome
- preserves step/run bindings for later flow-side reconciliation

#### `FailWork`

- transitions `Running -> Failed`
- records a terminal outcome
- preserves step/run bindings for later flow-side reconciliation

#### `CancelWork`

- transitions live work into `Canceled`
- records a terminal outcome
- preserves step/run bindings for later flow-side reconciliation

#### `CancelAllWork`

- bulk-cancels all live work for one `(identity, runtime, fence)` binding

### 4. Flow-run lifecycle

#### `StartFlowRun`

- creates a durable run entry
- requires at least one dispatch-capable member
- records flow identity
- seeds ordered steps and kernel maps, including an explicit per-step
  dispatch-mode assignment
- seeds per-step dispatch width to `0`; actual dispatch multiplicity is chosen
  later by `DispatchStep`
- increments `lifecycle.active_run_count`

#### `TrackRun`

- adds the run to tracked run / cancel-token / stream sets

#### `CompleteRun`

- transitions run to `Completed`
- sets completion presence
- drains any residual live work bound to that run
- clears any live task bindings to that run
- decrements active-run count

#### `FailRun`

- transitions run to `Failed`
- sets completion presence
- increments failure counters
- rewrites any remaining incomplete step state to terminal canceled step state
- drains any residual live work bound to that run as failed
- clears any live task bindings to that run
- decrements active-run count if terminal

#### `CancelRun`

- transitions run to `Canceled`
- sets completion presence
- rewrites any remaining incomplete step state to terminal canceled step state
- drains any residual live work bound to that run as canceled
- clears any live task bindings to that run
- decrements active-run count

#### `FailRunNoDispatchCapacity`

- transitions a running run to `Failed`
- applies only when dispatch-ready undispatched work remains but no
  dispatch-capable member exists
- sets completion presence
- increments failure counters
- rewrites any remaining incomplete step state to terminal canceled step state
- drains any residual live work bound to that run as failed
- clears any live task bindings to that run
- decrements active-run count

### 5. Step / branch / collection progress

#### `DispatchStep`

- requires at least one dispatch-capable member
- requires a dependency-satisfied step
- consults the stored dispatch mode for that step
- chooses actual dispatch multiplicity at dispatch time
- materializes step status `Dispatched`
- stores the chosen dispatch width on the step
- allocates one or more work items targeting that step

#### `MarkStepCompleted`

- transitions one step to `Completed`
- applies only to directly dispatched non-quorum steps
- drains any live work bound to that step as completed
- enables any newly satisfied dependent steps

#### `MarkStepFailed`

- transitions one step to `Failed`
- increments failure counters
- drains any live work bound to that step as failed
- if the failure threshold terminalizes the run, drains residual live work
  across the run as failed and rewrites the rest of the run's incomplete step
  state to terminal canceled step state

#### `MarkStepSkipped`

- transitions one step to `Skipped`
- drains any residual live work bound to that step as canceled

#### `MarkStepCanceled`

- transitions one step to `Canceled`
- drains any live work bound to that step as canceled

#### `ResolveAnyJoin`

- evaluates `DependencyMode::Any` joins against completed branch members
- transitions the join step into explicit machine-owned ready/pending state
- does not allocate or bind work yet
- does not itself allocate member work

#### `ResolveCollection`

- resolves collection completion for one step
- for `Quorum`, requires observed contribution count to meet the stored
  threshold before transitioning the step to `Completed`

#### `RecordCollectionContribution`

- records one observed contribution for a quorum collection step
- increments explicit machine-owned collection progress
- preserves the step in `Dispatched` until the stored threshold is met

### 6. Frames and loops

#### `OpenFrame`

- increments frame structure for a loop-capable run
- applies only when the run has no live bound work

#### `CloseFrame`

- reduces live frame structure for a loop-capable run
- applies only when the run has no live bound work and no live loop structure

#### `OpenLoop`

- increments loop structure for a loop-capable run
- applies only when the run has no live bound work

#### `AdvanceLoopIteration`

- increments loop iteration ledger count for a loop-capable run
- applies only when the run has no live bound work

#### `CloseLoop`

- reduces loop structure for a loop-capable run
- applies only when the run has no live bound work

### 7. Recovery / history / tasks

#### `RecordRestoreFailure`

- records restore-failure presence and reason
- clears kickoff pending state for the failed identity
- clears coordinator or external-spec presence if they referenced that identity

#### `ClearRestoreFailure`

- removes restore-failure state after repair

#### `AdvanceCheckpointVersion`

- increments continuity/checkpoint version for one identity

#### `AppendHistoryEvent`

- increments machine event sequence
- appends durable per-identity history count
- preserves per-identity `last_seq == event_count`

#### `OpenTask`

- creates a task-board entry

#### `UpdateTask`

- changes active task status
- attaches an active task only to a known non-terminal run
- clears the run binding once the bound run terminalizes

#### `CloseTask`

- terminalizes a task-board entry
- clears its live run binding

### 8. Top-level mob lifecycle

#### `StopMob`

- applies only while the mob is `Running`
- requires `active_run_count == 0`
- requires kickoff pending to be empty
- moves the top-level phase to `Stopped`
- clears `cleanup_pending`
- leaves only `CompleteMob`, `DestroyMob`, or stutter enabled

#### `CompleteMob`

- applies only while the mob is `Running` or `Stopped`
- requires `active_run_count == 0`
- requires kickoff pending to be empty
- moves the top-level phase to `Completed`
- clears `cleanup_pending`
- leaves only `DestroyMob` or stutter enabled

#### `DestroyMob`

- applies only while the mob is `Stopped` or `Completed`
- requires `active_run_count == 0`
- requires the roster to be empty
- requires pending spawn to be empty
- requires kickoff pending to be empty
- requires live work to be drained
- moves the top-level phase to `Destroyed`
- clears `cleanup_pending`
- leaves only stutter enabled
