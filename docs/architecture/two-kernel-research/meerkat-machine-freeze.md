# MeerkatMachine Freeze

Status: frozen target-state asset

This note closes top-level `M1 = MeerkatMachine`.

It is the full target-state freeze of `MeerkatMachine` as a single semantic
kernel. This is the asset to take into TLA+ if the goal is to prove the
machine we intend to cut over to, not merely the behavior of today's runtime.

The exact-current baseline remains separately frozen in
`meerkat-machine-exact-current-freeze.md`.

The frozen target vocabulary is defined in `meerkat-machine-glossary.md`.

## Purpose

This note exists to prevent a common failure mode:

- exact-current observations get mistaken for the intended architecture
- target-state cleanup gets mixed into “what the code already does”
- the TLA+ model quietly proves the wrong thing

This freeze is the opposite of that.

It states the target single-machine design explicitly, including the places
where it intentionally differs from the exact-current codebase.

## Target Boundary

`MeerkatMachine` owns the full semantics of one interactive runtime:

- binding and hidden execution continuity
- lifecycle and control-plane authority
- admitted input truth and queue discipline
- input completion-waiter truth
- turn execution and cancellation posture
- async operations, barrier coordination, and `wait_all`
- detached wake and continuation injection
- peer trust, normalization, queueing, replay, and typed admission
- external tool-surface truth, mutation, and recovery
- keep-alive / drain lifecycle
- whole-machine recovery and reconstruction

It does not own:

- durable Mob identity, generation, or fencing
- roster, topology intent, or flow planning
- scheduler / occurrence semantics
- operator projections as semantic truth

## Target Design Commitments

These are the hard commitments of the target machine.

### 1. One semantic owner per fact

Every semantic fact inside the single-session runtime has one owner inside
`MeerkatMachine`.

Helpers, adapters, stores, caches, channels, snapshots, and projections may
exist, but none of them become parallel semantic authorities.

### 2. One admission path for all executable work

All executable work enters through the admitted-input ledger:

- surface prompts
- steered/resume inputs
- typed peer work
- detached background continuation inputs

There is no second execution path for peer ingress, detached wake, or tool
surface side effects.

### 3. Completion waiters are explicit machine state

Input-owned completion waiters are not left as an implicit support carrier.
They are modeled explicitly as part of the machine because they have real
semantic coupling to lifecycle, recovery, and exactly-once terminalization.

### 4. Barrier truth derives from async-op truth

`WaitingForOps`, barrier identity, barrier satisfaction, and `wait_all` are all
derived from the async-op region. There are no shadow booleans elsewhere.

### 5. Detached wake is integrated, not dual-pathed

The target machine has one detached-wake story:

- background terminal completion enters through the async-op region
- quiescence is checked against machine state
- continuation injection is emitted as a machine effect

The legacy latch path is not part of the target machine.

### 6. Recovery reconstructs canonical truth

Recovery rebuilds the machine from authoritative records:

- binding and hidden continuity
- input ledger and queue state
- completion waiters
- turn state
- async operations
- peer ingress backlog and trust
- tool-surface state
- drain state

Recovery never trusts convenience projections as semantic inputs.

### 7. Lifecycle commands are machine inputs

The target machine explicitly owns the semantics of:

- `RetireRuntime`
- `StopRuntime`
- `ResetRuntime`
- `RecoverRuntime`
- `RecycleRuntime`
- `DestroyRuntime`
- `InterruptCurrentRun`
- `InterruptYielding`
- `CancelAfterBoundary`

These are not implicit helper paths, comments, or authority-local folklore.

## Target Internal Regions

The target `MeerkatMachine` state is:

```text
MeerkatMachine =
  binding
  + control
  + inputs
  + completion
  + turn
  + ops
  + peer
  + tools
  + drain
```

### `binding`

- `session_id`
- `runtime_id`
- `driver_present`
- `attachment_live`
- `epoch_id`
- `cursor_state`

### `control`

- `phase`
- `current_run_id`
- `pre_run_phase`
- `wake_pending`
- `process_pending`
- `pending_control`

### `inputs`

- admitted input set
- per-input metadata
- queue lane
- steer lane
- staged contributors
- per-input lifecycle
- terminal outcomes

### `completion`

- `waiting_inputs`
- `waiter_count_by_input`
- `resolved_inputs`

This is explicit target machine state, not an unmodeled support carrier.

### `turn`

- active run
- primitive kind
- turn phase
- `tool_calls_pending`
- `pending_op_refs`
- `barrier_operation_ids`
- `barrier_satisfied`
- `cancel_after_boundary`
- retry / extraction state
- run terminal outcome

### `ops`

- operation registry
- operation kind / status
- watcher and progress counts
- terminal buffering and ordering
- `wait_all`
- peer-ready state

### `peer`

- trust set
- auth mode
- classified ingress backlog
- terminalized peer-item lineage
- admitted peer-item lineage
- request/response correlation
- typed-admission readiness
- replayable pending peer items

### `tools`

The target tools region is now best read as one top-level region with two
tightly coupled internal subregions:

- `tool_visibility`
- `tool_surface`

The current frozen bullets below remain the compact target summary. The active
target-delta note records the likely growth needed after absorbing the richer
upstream visibility/catalog ownership baseline:

- `meerkat-tools-target-delta.md`

- known surfaces
- visible surfaces
- staged intents
- pending mutation lineage
- inflight call counts
- published snapshot alignment

### `drain`

- drain mode
- task phase
- suppression state
- respawn state

## Target Lifecycle Semantics

### `InterruptCurrentRun`

- hard-cancel request
- is observed at the next machine-visible cancellation boundary
- may emit executor-facing control effects as part of enforcement, but the
  semantic request belongs to the machine itself
- always remains part of the top-level machine alphabet

### `InterruptYielding`

- cooperative interrupt request
- never silently upgrades into hard cancel
- is a machine input/effect even when one implementation lowering uses
  executor control
- does not imply or require a separate session-layer API boundary

### `CancelAfterBoundary`

- top-level machine input
- legal only when a run is active
- preserved through the turn region until boundary drain resolves
- not excluded from the target machine just because the current code does not
  lower it fully yet

### `RetireRuntime`

- forbids new ordinary admission
- permits already-admitted work to drain according to machine queue and turn
  semantics
- when settled, no current-run binding remains
- no input-owned completion waiter may survive settled `Retired`

### `StopRuntime`

- clears input-owned queued work and input completion waiters
- permits ops-owned `wait_all` to remain until outstanding operations settle
- when settled, queue and steer projections are empty

### `ResetRuntime`

- abandons queued and staged input work
- clears input-owned completion waiters
- preserves no current-run binding
- rotates `binding.epoch_id`
- preserves only the machine-owned persistent regions that are explicitly
  designated durable across reset:
  - trust set
  - tool-surface visibility baseline

### `RecycleRuntime`

- preserves admitted pending work intentionally
- preserves `binding.epoch_id`
- reconstructs queues, completion waiters, and active wait state from canonical
  records

### `RecoverRuntime`

- preserves `binding.epoch_id`
- reconstructs the full machine from authoritative records
- includes pending peer ingress replay and tool-surface recovery as target
  semantics

### `DestroyRuntime`

- removes the runtime binding entirely
- clears all input-owned queued work and completion waiters immediately
- terminates outstanding lifecycle obligations
- destroys drain ownership
- is terminal for the machine instance; rebinding after destroy belongs to a
  fresh machine `Init`, not to `PrepareBindings`
- once `Destroyed` is reached, no further semantic machine transition is legal;
  only stutter remains

## Target Input / Effect Alphabet

The target alphabet is intentionally broader than the exact-current alphabet.

### Inputs

- `PrepareBindings`
- `RegisterSession`
- `RegisterSessionWithExecutor`
- `InterruptCurrentRun`
- `InterruptYielding`
- `CancelAfterBoundary`
- `StopRuntime`
- `RetireRuntime`
- `ResetRuntime`
- `RecoverRuntime`
- `RecycleRuntime`
- `DestroyRuntime`
- `AdmitQueuedInput`
- `AdmitSteeredInput`
- `AdmitPeerIngress`
- `ReplayPeerIngress`
- `UpdatePeerIngressContext`
- `RegisterPendingOps`
- `WaitAllRequested`
- `WaitAllSatisfied`
- `OpsBarrierSatisfied`
- `ToolCallsResolved`
- `BoundaryContinue`
- `BoundaryComplete`
- `CancellationObserved`
- `RetryRequested`
- `RegisterTrustedPeer`
- `UnregisterTrustedPeer`
- `ReceivePeerIngress`
- `DrainPeerIngress`
- `StageAdd`
- `StageRemove`
- `StageReload`
- `ApplyBoundary`
- `PendingSucceeded`
- `PendingFailed`
- `FinalizeRemovalClean`
- `FinalizeRemovalForced`
- `EnsureDrainRunning`
- `TaskSpawned`
- `TaskExited`
- `AbortDrain`

### Effects

- `WakeRuntimeLoop`
- `ExecutorControl(CancelCurrentRun)`
- `ExecutorControl(InterruptYielding)`
- `ExecutorControl(StopRuntimeExecutor)`
- `ResolveCompletionWaiters`
- `WaitAllRequested`
- `InjectDetachedWakeContinuation`
- `PublishToolSurfaceSnapshot`
- `EmitExternalToolUpdate`
- `SpawnDrainTask`
- `AbortDrainTask`
- `PersistRecoverySnapshot`

## Target Invariants

The target machine must preserve at least these invariants:

1. At most one active run exists for one live binding.
2. `control.current_run_id`, `inputs.current_run_id`, and `turn.active_run_id`
   agree whenever any of them is present.
3. `completion.waiting_inputs` is a subset of admitted non-terminal inputs.
4. Input completion waiters resolve exactly once.
5. Queue, steer, staged, applied, and terminal lifecycle states are pairwise
   legal with respect to the admitted-input ledger.
6. No terminal input reappears in queue or steer projections.
7. `WaitingForOps` implies a non-empty pending-op set.
8. Barrier op ids are a subset of pending op ids.
9. `ToolCallsResolved` is legal only after barrier satisfaction.
10. Detached background completions inject at most one continuation per
    terminal event.
11. Every peer work item is terminalized exactly once as either admitted work
    or failed/drained peer lineage.
12. Peer replay never duplicates already terminalized peer work.
13. Trust membership changes only through the canonical peer region transitions.
14. Visible tool surfaces equal the last applied machine-owned tool-surface
    state.
15. Staged tool-surface state cannot outrun the last applied boundary.
16. Drain ownership exists only for a live runtime binding and a drain mode
    that requires it.
17. `ResetRuntime` rotates `epoch_id`; `RecoverRuntime` and `RecycleRuntime`
    do not.

## Target Decisions That Resolve Previous Open Questions

These are now frozen for the target machine:

- completion waiters are an explicit machine subregion
- `cancel_after_boundary` is part of the target machine alphabet
- peer-ingress backlog is recoverable and replayable
- router-only trust mutation helpers are not part of the target machine
- tool-surface recovery is machine-owned, not left to a second out-of-band
  store protocol
- all drain modes are machine-owned, not only `PersistentHost`
- `ResetRuntime` rotates `epoch_id`
- `RecycleRuntime` preserves `epoch_id`
- detached wake uses one integrated machine path; the legacy latch path is not
  part of the target machine

## Delta From Exact-Current Baseline

Relative to `meerkat-machine-exact-current-freeze.md`, the target machine
promotes these items from “excluded or compatibility” into target semantics:

- `CancelAfterBoundary`
- explicit completion-waiter subregion
- replayable peer-ingress backlog
- full tool-surface recovery
- all drain modes
- integrated detached-wake path with no legacy fallback
- reset-driven epoch rotation as a firm semantic rule

Relative to the exact-current implementation, the target machine also removes:

- driver-local queue truth as an independent semantic carrier
- raw router trust mutation as canonical authority
- silent reliance on compatibility fallback paths as part of the final model

## TLA+ Posture

This is the asset to use for target-state TLA+ work.

The proof handoff is captured in `meerkat-machine-proof-obligations.md`.
The canonical transition reference is
`meerkat-machine-transition-catalog.md`.
The canonical state schema is `meerkat-machine-state-schema.md`.
Canonical derived predicates are defined in
`meerkat-machine-derived-predicates.md`.
Alphabet and region coverage is checked in
`meerkat-machine-coverage-matrix.md`.
Fairness assumptions are fixed in `meerkat-machine-fairness-assumptions.md`.
An executable target-state TLC scaffold now lives in `tla/`.
That scaffold now passes both base and widened bounded TLC runs against a
strengthened safety-invariant set.

The right modeling sequence is:

1. keep `meerkat-machine-exact-current-freeze.md` as the implementation
   baseline
2. model this target-state `MeerkatMachine`
3. prove the target machine
4. compare exact-current implementation against the target machine explicitly
   instead of smearing the two together

## Freeze Decision

Top-level `M1 = MeerkatMachine` is now frozen as the single target-state
machine we intend to prove.

The next blocking milestone is no longer “finish defining Meerkat.” It is:

1. write the TLA+ model for this target machine
2. use the exact-current baseline as comparison evidence
3. review the delta before any write-side cutover
