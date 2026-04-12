# MeerkatMachine Proof Obligations

Status: frozen target-state proof handoff

This note turns `meerkat-machine-freeze.md` into a TLA+-ready proof package.

Use it together with:

- `meerkat-machine-freeze.md` as the target semantic machine
- `meerkat-machine-exact-current-freeze.md` as the implementation baseline
- `meerkat-machine-state-schema.md` as the canonical state and `Init` schema
- `meerkat-machine-derived-predicates.md` as the canonical derived-predicate
  layer
- `meerkat-input-effect-alphabet.md` as the exact-current action/effect source
- `meerkat-lowering-map.md` as the implementation comparison map
- `meerkat-machine-transition-catalog.md` as the canonical target transition
  reference
- `meerkat-machine-coverage-matrix.md` as the target alphabet / region
  coverage check
- `meerkat-machine-glossary.md` for frozen target terminology
- `meerkat-machine-fairness-assumptions.md` for allowed liveness assumptions

## Purpose

This note answers a narrower question than the freeze itself:

- what exactly must the TLA+ model represent?
- what must be proved as safety?
- what must be proved as liveness?
- what implementation deltas must be reviewed after proof?

It exists so the target freeze is not merely descriptive. It is the explicit
handoff to proof work.

## Modeling Boundary

The TLA+ model covers one `MeerkatMachine` instance with these regions:

- `binding`
- `control`
- `inputs`
- `completion`
- `turn`
- `ops`
- `peer`
- `tools`
- `drain`

It excludes:

- durable Mob identity / generation / fencing
- topology intent and orchestration
- scheduler / occurrence semantics
- UI or operator projections as semantic truth

## Finite Abstraction Rules

The proof model should stay finite without hiding semantic commitments.

### Identity domains

Model small finite sets for:

- `InputId`
- `RunId`
- `OpId`
- `PeerId`
- `SurfaceId`
- `EpochId`

`EpochId` only needs ordering and distinct rotation, not UUID shape.

### Payload domains

Do not model full prompt text, tool payloads, or peer content.
Model only the distinctions that affect semantics:

- ordinary queued input
- steered input
- peer work
- detached continuation work
- tool-surface mutation intent

### Effects

Model effects as explicit machine outputs or post-state obligations, not as
opaque side comments.

At minimum:

- `WakeRuntimeLoop`
- executor control outputs
- `ResolveCompletionWaiters`
- `InjectDetachedWakeContinuation`
- tool-surface publication outputs
- drain spawn / abort outputs
- persistence / recovery outputs

## Action Families To Model

The target model should expose these grouped actions.

### Binding and lifecycle

- `PrepareBindings`
- `RegisterSession`
- `RegisterSessionWithExecutor`
- `RetireRuntime`
- `StopRuntime`
- `ResetRuntime`
- `RecoverRuntime`
- `RecycleRuntime`
- `DestroyRuntime`

### Turn and cancellation

- `InterruptCurrentRun`
- `InterruptYielding`
- `CancelAfterBoundary`
- `BoundaryContinue`
- `BoundaryComplete`
- `CancellationObserved`
- `RetryRequested`

### Admission and completion

- `AdmitQueuedInput`
- `AdmitSteeredInput`
- `ResolveCompletionWaiters`

### Async ops and barrier

- `RegisterPendingOps`
- `WaitAllRequested`
- `WaitAllSatisfied`
- `OpsBarrierSatisfied`
- `ToolCallsResolved`
- `PendingSucceeded`
- `PendingFailed`

### Peer ingress

- `RegisterTrustedPeer`
- `UnregisterTrustedPeer`
- `ReceivePeerIngress`
- `AdmitPeerIngress`
- `ReplayPeerIngress`
- `DrainPeerIngress`
- `UpdatePeerIngressContext`

### Tool surface

- `StageAdd`
- `StageRemove`
- `StageReload`
- `ApplyBoundary`
- `FinalizeRemovalClean`
- `FinalizeRemovalForced`

### Drain lifecycle

- `EnsureDrainRunning`
- `TaskSpawned`
- `TaskExited`
- `AbortDrain`

## Safety Obligations

These are the minimum safety properties the target machine must satisfy.

### Core ownership

1. At most one active run exists for one live binding.
2. `control.current_run_id`, `inputs.current_run_id`, and `turn.active_run_id`
   agree whenever any is present.
3. `binding` exists iff lifecycle is not destroyed.
4. `drain` ownership cannot exist without a live binding and a drain-requiring
   mode.

### Input and completion

5. Every waiting completion input is admitted and non-terminal.
6. Input completion waiters resolve exactly once.
7. No terminal input reappears in queue or steer projections.
8. Queue, steer, staged, applied, and terminal lifecycle states are pairwise
   legal with respect to the admitted-input ledger.
9. `Retired`, `Stopped`, and `Destroyed` settled states do not retain illegal
   queued or current-run bindings.

### Turn / ops / barrier

10. `WaitingForOps` implies a non-empty pending-op set.
11. Barrier op ids are a subset of pending op ids.
12. `ToolCallsResolved` is legal only after barrier satisfaction.
13. `CancelAfterBoundary` does not disappear before the boundary resolves.
14. `wait_all` identity and pending carrier identity stay aligned.

### Detached wake

15. One terminal detached background completion yields at most one continuation
    admission.
16. Detached continuation injection is legal only from quiescent machine state
    or through an explicit deferred path.

### Peer ingress

17. Every peer work item either fails before admission or becomes an ordinary
    admitted input.
18. Trust membership changes only through canonical peer transitions.
19. Peer replay never duplicates already admitted peer work.

These obligations require machine-owned peer lineage. The proof model should
therefore carry explicit terminalized/admitted peer-item state rather than
trying to infer replay uniqueness from transient backlog shape alone.

### Tool surface

20. Visible tool surfaces equal the last applied machine-owned surface state.
21. Staged tool-surface state cannot outrun the last applied boundary.
22. Inflight tool-call state is compatible with machine-owned surface phase.

The active target-delta review for this region is captured in:

- `meerkat-tools-target-delta.md`

That review is expected to expand the obligation set so `tools` is treated as
`tool_visibility + tool_surface` rather than only surface lifecycle.

### Epoch continuity

23. `ResetRuntime` rotates `epoch_id`.
24. `RecoverRuntime` and `RecycleRuntime` preserve `epoch_id`.
25. `DestroyRuntime` is terminal for the machine instance; no later
    transition may recreate a live binding from `Destroyed`.
26. `Destroyed` is a stutter-only state for the machine instance; no semantic
    action other than stutter is legal after destruction.

## Liveness Obligations

These should be modeled explicitly as conditional liveness obligations, not
smuggled in as safety.

1. If a queued or steered input is admitted and no stronger lifecycle command
   destroys it, it eventually reaches terminal input state.
2. If `wait_all` is active and all referenced operations eventually settle,
   `wait_all` eventually resolves.
3. If detached wake conditions are satisfied and the machine becomes quiescent,
   continuation injection eventually occurs exactly once.
4. If `CancelAfterBoundary` is set and the active turn reaches boundary drain,
   cancellation is eventually observed before new turn work begins.
5. If a trusted peer item remains replayable and the machine continues making
   progress, it is eventually either admitted or failed.
6. If a tool-surface mutation is staged and boundaries continue completing,
   the visible surface state eventually converges to the staged intent.

Any stronger liveness claim should be added only if it is a deliberate target
commitment, not because it sounds desirable.

## Refinement / Delta Review Obligations

After the target machine is modeled, implementation review must explicitly
cover the deltas against `meerkat-machine-exact-current-freeze.md`.

The proof review must call out:

- `CancelAfterBoundary` promoted from excluded to first-class target input
- completion waiters promoted from support carrier to explicit machine region
- peer-ingress backlog promoted from non-replayable exact-current seam to
  replayable target state
- tool-surface recovery promoted from exact-current reconstruction to
  machine-owned target recovery
- all drain modes promoted into the target machine
- detached wake reduced to one integrated target path
- reset-driven epoch rotation promoted into firm target semantics

These are not proof bugs. They are intended target deltas and should be
reviewed as such.

## Recommended Proof Sequence

1. Prove core lifecycle, binding, input, and completion safety.
2. Add turn / ops / barrier semantics and prove the coupling obligations.
3. Add detached wake and continuation injection.
4. Add peer ingress, trust, replay, and typed admission.
5. Add tool-surface lifecycle and recovery.
6. Add drain lifecycle.
7. Re-run the full safety set.
8. Add conditional liveness.
9. Compare the target proof surface against the exact-current baseline.

## Acceptance For "Ready To Prove"

`MeerkatMachine` is ready for TLA+ implementation when all of the following are
true:

- the target freeze remains the single semantic source of truth
- this proof-obligations note matches the freeze exactly
- the exact-current baseline is stable enough to serve as implementation
  comparison evidence
- no active supporting doc still treats target decisions as unresolved

That state is now reached.
