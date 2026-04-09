# MeerkatMachine Transition Catalog

Status: frozen target-state transition handoff

This note is the canonical transition catalog for the target-state
`MeerkatMachine`.

Use it together with:

- `meerkat-machine-freeze.md` for target state and commitments
- `meerkat-machine-proof-obligations.md` for proof scope and obligations
- `meerkat-machine-exact-current-freeze.md` for implementation comparison

## Purpose

The top-level freeze defines:

- the machine boundary
- the machine state
- the alphabet
- the invariants

This note adds the missing operational layer:

- which transitions exist
- what they are allowed to read
- what they are allowed to update
- which effects they may emit

That makes the `MeerkatMachine` freeze package suitable for direct TLA+
encoding as one machine rather than as a bag of state variables plus informal
intent.

## Transition Form

Every target transition should be modeled in this normal form:

1. input or internally triggered machine action
2. preconditions over current machine state
3. state updates over one or more machine regions
4. emitted effects, if any
5. preserved regions, if not updated

The machine does not admit “silent semantic side paths.” If a meaningful
semantic state change occurs, it must appear as a named machine transition.

## Transition Families

### 1. Binding Lifecycle

#### `PrepareBindings`

- requires the pre-binding initializing state
- creates or reuses the hidden runtime skeleton
- initializes `binding`
- does not create queued work by itself
- may emit persistence effects if the machine requires binding materialization

#### `RegisterSession`

- requires a valid binding
- sets lifecycle into a registered non-attached state
- publishes an initial control state with no active run
- may initialize drain mode but does not require an active drain task

#### `RegisterSessionWithExecutor`

- requires a valid binding
- marks executor attachment as live
- enables executor-facing control effects
- may trigger drain startup if the selected drain mode requires it

### 2. Admission

#### `AdmitQueuedInput`

- requires no terminal record for the input id
- inserts input metadata into the admitted-input ledger
- appends the input to the ordinary queue lane
- creates or updates the completion subregion if waiters are attached
- sets control wake intent
- emits `WakeRuntimeLoop`

#### `AdmitSteeredInput`

- requires no terminal record for the input id
- inserts input metadata into the admitted-input ledger
- appends the input to the steer lane
- creates or updates the completion subregion if waiters are attached
- sets both wake and process intent
- emits `WakeRuntimeLoop`

#### `AdmitPeerIngress`

- requires a classified and admissible peer work item
- consumes one replayable or freshly received peer item from the peer region
- creates an ordinary admitted input in the input ledger
- may place it in queue or steer according to the peer admission rule
- must not create a second execution path outside the admitted-input ledger

#### `ReplayPeerIngress`

- requires replayable peer backlog in the peer region
- re-materializes admissible peer items through the ordinary peer admission
  path
- must not duplicate already admitted peer work

### 3. Run Start And Control

#### `WakeRuntimeLoop`

- is an emitted effect, not a separate semantic owner
- may be consumed by a driver or executor lowering
- must not itself mutate semantic state beyond the machine transition that
  emitted it

#### `BeginRun`

- requires queued or steered admitted work and no active run
- reserves a `RunId`
- moves `control.current_run_id`, `inputs.current_run_id`, and
  `turn.active_run_id` into alignment
- stages the contributing inputs
- removes consumed contributors from queue or steer projections
- enters an active turn phase

#### `InterruptCurrentRun`

- requires an active run
- records a hard-cancel request in machine-owned turn/control state
- may emit `ExecutorControl(CancelCurrentRun)` as an enforcement effect
- does not silently erase the request before a machine-visible cancellation
  boundary

#### `InterruptYielding`

- requires an active run or an attached cooperative execution seam
- records a cooperative interrupt request in machine-owned state
- may emit `ExecutorControl(InterruptYielding)`
- never upgrades itself into hard cancel without a separate machine transition

#### `CancelAfterBoundary`

- requires an active run
- marks the turn region so boundary drain becomes terminal rather than
  continuing normally
- remains machine-owned until boundary resolution consumes it

### 4. Turn / Ops / Barrier

#### `RegisterPendingOps`

- requires an active run entering waiting-for-ops posture
- records pending op refs and barrier op ids inside the turn/ops regions
- may initialize `wait_all` identity if a barrier wait is being started

#### `WaitAllRequested`

- creates or updates machine-owned `wait_all`
- binds the machine pending carrier identity to the ops region identity
- does not resolve itself synchronously unless no pending ops remain

#### `WaitAllSatisfied`

- consumes a machine-owned `wait_all` instance that has become satisfied
- resolves the pending wait carrier identity in lock-step with the ops region
- may enable `OpsBarrierSatisfied` or release a lifecycle path that is waiting
  on op settlement
- must not be modeled as a side effect with no corresponding machine
  transition

#### `PendingSucceeded` / `PendingFailed`

- update the op registry
- preserve op identity and terminal ordering
- may trigger barrier satisfaction checks
- may trigger detached-wake continuation eligibility for background work

#### `OpsBarrierSatisfied`

- requires all barrier op ids to be terminal
- sets barrier satisfaction in the turn region
- may move the turn into post-barrier boundary drain

#### `ToolCallsResolved`

- legal only after barrier satisfaction
- clears tool-call waiting posture
- advances the turn into boundary drain or completion

#### `BoundaryContinue`

- consumes one boundary drain step
- either resumes execution or transitions toward cancellation/terminalization
- applies any staged cross-region boundary effects that are legal at continue
  time

#### `BoundaryComplete`

- completes boundary drain
- updates per-input lifecycle and boundary sequence truth
- finalizes staged boundary-applied effects
- may terminalize the run if no continuation path remains

#### `CancellationObserved`

- records that cancellation became effective at the turn boundary
- transitions the run toward terminal canceled posture

#### `RetryRequested`

- legal only from machine-owned retryable turn states
- increments retry / extraction state
- re-enters the turn loop without violating current-run identity coherence

### 5. Completion

#### `ResolveCompletionWaiters`

- resolves or terminates input-owned completion waiters exactly once
- removes the input from `completion.waiting_inputs`
- updates the per-input completion resolution state
- must not resolve inputs that were never admitted

### 6. Detached Wake

#### `InjectDetachedWakeContinuation`

- requires a terminal detached background completion and quiescent machine
  state, or an explicit deferred detached-wake path
- materializes continuation work through the admitted-input ledger
- must emit at most one continuation per eligible terminal background event
- does not use a second execution path outside ordinary admission

### 7. Peer Region

#### `RegisterTrustedPeer`

- updates the machine-owned trust set
- may affect later admissibility of peer items
- does not mutate already admitted input truth

#### `UnregisterTrustedPeer`

- removes trust membership
- affects future peer admission decisions
- does not silently invalidate already admitted input truth

#### `ReceivePeerIngress`

- normalizes incoming peer items into machine-owned classified peer state
- records trust/auth snapshot and correlation facts
- requires that the item is not already terminalized in peer lineage
- may place items into replayable or immediately drainable peer backlog

#### `DrainPeerIngress`

- consumes classified peer backlog items through the peer region
- either rejects/fails them or emits ordinary admission via `AdmitPeerIngress`
- terminalizes drained non-admitted peer items in peer-owned lineage
- does not bypass the input ledger

#### `UpdatePeerIngressContext`

- updates machine-owned peer/drain context that affects ingress and drain
  behavior
- must not become a semantic side path for executing peer work

### 8. Tool Surface

#### `StageAdd` / `StageRemove` / `StageReload`

- update staged tool-surface intent
- `StageReload` requires an already applied machine-owned surface state
- preserve the last applied visible surface state until boundary application
- may record pending mutation lineage

#### `ApplyBoundary`

- legal only at a boundary application point
- applies staged tool mutations into visible machine-owned surface truth
- may emit `PublishToolSurfaceSnapshot`
- may emit `EmitExternalToolUpdate`

#### `FinalizeRemovalClean` / `FinalizeRemovalForced`

- complete removal lifecycle
- reconcile inflight and staged mutation lineage
- preserve machine-owned visibility invariants

### 9. Drain Lifecycle

#### `EnsureDrainRunning`

- requires a live binding and a drain-requiring mode
- records that a drain task is required
- may emit `SpawnDrainTask`

#### `TaskSpawned`

- closes a pending spawn obligation
- marks the drain task as live

#### `TaskExited`

- records that the drain task exited
- may require respawn according to drain mode and lifecycle state
- must not leave orphan drain ownership for a dead binding

#### `AbortDrain`

- requests drain task shutdown
- may emit `AbortDrainTask`
- reconciles drain phase against lifecycle state

### 10. Recovery / Recycle / Reset / Retire / Stop / Destroy

#### `RetireRuntime`

- requires a live binding
- forbids new ordinary admission
- preserves already-admitted work according to machine queue/turn rules
- eventually settles with no current-run binding and no input-owned completion
  waiters
- may preserve ops-owned `wait_all` until outstanding ops settle

#### `StopRuntime`

- requires a live binding
- clears input-owned queued work and completion waiters
- transitions the runtime into stopped lifecycle posture
- may emit `ExecutorControl(StopRuntimeExecutor)` when an attached executor
  lowering must be driven to a stopped state
- may preserve ops-owned `wait_all` until outstanding ops settle
- leaves no settled queue or current-run residue

#### `ResetRuntime`

- abandons queued and staged input work
- clears input-owned completion waiters
- rotates `epoch_id`
- preserves only explicitly durable machine-owned regions

#### `RecoverRuntime`

- rebuilds all machine regions from authoritative records
- preserves `epoch_id`
- includes peer backlog replay and tool-surface recovery as machine semantics

#### `RecycleRuntime`

- preserves intentionally durable pending work
- preserves `epoch_id`
- reconstructs queues, completion waiters, and active wait state from
  authoritative machine records

#### `DestroyRuntime`

- requires a live binding
- removes the runtime binding
- clears queued work and input-owned completion waiters immediately
- terminates lifecycle obligations tied to the destroyed runtime
- removes drain ownership
- is terminal for the machine instance; later rebinding is outside this
  machine and belongs to a fresh `Init`
- after `DestroyRuntime`, no further semantic transition is legal for this
  machine instance

#### `PersistRecoverySnapshot`

- is emitted after transitions that change machine-owned reconstructable truth
- captures the canonical state required for later `RecoverRuntime` or
  `RecycleRuntime`
- is driven by machine-owned lifecycle and state transitions, not by ad hoc
  helper snapshots outside the machine

## Cross-Region Transition Requirements

Some obligations are not local to one region and must be modeled as joint
updates.

### Run identity coherence

Whenever a transition creates, mutates, or clears active run state, it must
maintain coherence across:

- `control.current_run_id`
- `inputs.current_run_id`
- `turn.active_run_id`

### Completion / lifecycle coupling

Lifecycle transitions that destroy, reset, stop, or retire input work must
explicitly account for:

- queue and steer projections
- per-input lifecycle
- input-owned completion waiters
- the non-equivalence of input waiters and ops-owned `wait_all`

### Barrier / tool coupling

Boundary and tool transitions must preserve:

- barrier satisfaction legality
- `ToolCallsResolved` legality
- boundary-apply ordering for staged tool mutations

### Recovery completeness

`RecoverRuntime` and `RecycleRuntime` must be modeled as whole-machine
reconstruction actions, not as region-by-region best-effort helpers.

## Transition Catalog Acceptance

This catalog is considered complete enough for target-state TLA+ work when:

- every target alphabet item is covered by one transition family
- no region relies on an uncataloged semantic side path
- cross-region obligations are explicit where required

That state is now reached.
