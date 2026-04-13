# Meerkat Turn / Ops / Barrier Freeze

Status: frozen exact-current-state asset

This note closes `K3` from `meerkat-cutover-checklist.md`.

It captures the exact current live Meerkat semantics for turn / ops / barrier
coupling at the turn-execution boundary. It freezes the current relationship
between:

- `TurnExecutionAuthority`
- the ops lifecycle registry
- the live runner loop
- the joined `MeerkatMachine` validator

It does **not** claim a globally atomic snapshot of turn phase and operation
terminalization. It freezes the exact coupling the live code already owns.

## Scope

This freeze covers:

- `WaitingForOps` turn semantics
- registration of pending async operations
- barrier-operation tracking and satisfaction
- the handoff from `wait_all` back into turn execution
- the boundary from `WaitingForOps` into `DrainingBoundary`
- the exact joined validator rules that are safe on the current snapshot
  surface

This freeze does **not** claim:

- that arbitrary joined turn + ops snapshots are fully atomic
- that operation status and turn phase can be compared without regard to the
  authority-owned wait/barrier seam
- that `cancel_after_boundary` is fully explained by the barrier model alone;
  its live lowering exists, but its full semantics remain frozen under
  `meerkat-interrupt-freeze.md`

## Exact current semantics

### 1. Pending ops only exist in `WaitingForOps`

Exact current live behavior:

- the turn authority accepts `RegisterPendingOps` only in `WaitingForOps`
- `tool_calls_pending` must already be greater than zero
- registering pending ops stores:
  - the full pending op set
  - the barrier-op subset
  - `has_barrier_ops`
  - `barrier_satisfied = !has_barrier_ops`

The exact current machine rule is therefore:

- pending operation ids are only valid while `turn_phase = WaitingForOps`

### 2. Barrier truth is a subset relation, not a parallel registry

Exact current live behavior:

- barrier op ids are not an independent second op ledger
- they are the barrier subset of the pending operation set
- `OpsBarrierSatisfied` is only valid when:
  - the run id matches
  - the barrier is not already satisfied
  - the reported operation ids exactly match the authority-owned barrier id set

The exact current machine rule is therefore:

- every barrier op id must also be a pending op id
- barrier satisfaction is the authority-owned gate that allows
  `ToolCallsResolved`

### 3. The live runner lowers barrier waiting through the ops registry

Exact current live behavior:

- the live runner asks the turn authority for `barrier_op_ids()`
- when that set is non-empty, it calls `ops_lifecycle.wait_all(&run_id, &ids)`
- the returned obligation is fed through the generated
  `protocol_ops_barrier_satisfaction` helper
- only then does the runner submit `ToolCallsResolved`

So the current live coupling is:

- turn authority defines what the barrier is
- ops lifecycle owns the async wait
- the generated barrier protocol bridges the two

### 4. `ToolCallsResolved` is blocked until the barrier is satisfied

Exact current live behavior:

- `ToolCallsResolved` in `WaitingForOps` requires:
  - matching run id
  - `tool_calls_pending > 0`
  - pending ops present
  - `barrier_satisfied = true`
- when accepted, it:
  - clears pending op refs
  - clears barrier ids
  - clears `has_barrier_ops`
  - restores `barrier_satisfied = true`
  - increments the boundary count
  - emits `BoundaryApplied`
  - moves to `DrainingBoundary`

### 5. The joined Meerkat validator freezes only the safe coupling facts

The exact current joined validator now freezes the following rules:

- `WaitingForOps` with `tool_calls_pending = 0` is invalid
- pending op ids outside `WaitingForOps` are invalid
- barrier ids without `has_barrier_ops` are invalid
- `has_barrier_ops` with an empty barrier-op set is invalid
- `!has_barrier_ops && !barrier_satisfied` is invalid
- every pending op id must exist in the ops registry snapshot
- every barrier op id must exist in the ops registry snapshot
- every barrier op id must also appear in the pending-op set

This is the exact current safe joined rule set. It intentionally stops short of
claims that would require a fully atomic cross-crate snapshot.

## Exact-vs-target-state classification

What is frozen as exact current live behavior:

- the authority-owned relationship between pending ops and barrier ops
- the `wait_all` lowering through the ops lifecycle registry
- the `OpsBarrierSatisfied -> ToolCallsResolved -> DrainingBoundary` handoff
- the joined validator rules above

What is explicitly **not** frozen as exact current live behavior:

- a stronger “all barrier ops terminal in the same snapshot where the turn is
  already advanced” rule
- any claim that detached-wake/continuation behavior is already fully absorbed
  into this barrier model

Those exclusions are intentional. They keep this freeze aligned to the live
code instead of a nicer but false whole-machine story.

## Proof inventory

Authority proofs:

- `all_barrier_ops_block_tool_calls_resolved_until_satisfied`
- `mixed_barrier_and_detached_ops_still_block_until_barrier_satisfied`
- `ops_barrier_satisfied_rejected_when_already_satisfied`
- `ops_barrier_satisfied_wrong_run_id_rejected`
- `barrier_satisfied_reset_after_tool_calls_resolved`

Live lowering proof:

- the runner loop in `meerkat-core/src/agent/state.rs` lowers
  `barrier_op_ids() -> wait_all -> protocol_ops_barrier_satisfaction ->
  ToolCallsResolved`

Joined Meerkat proof:

- `validate_meerkat_machine_snapshot_reports_turn_ops_barrier_violations`

## Reviewer Verification Lane

The narrow verification lane for this freeze asset is:

- `cargo test -p meerkat-core --lib all_barrier_ops_block_tool_calls_resolved_until_satisfied`
- `cargo test -p meerkat-core --lib mixed_barrier_and_detached_ops_still_block_until_barrier_satisfied`
- `cargo test -p meerkat-core --lib ops_barrier_satisfied_rejected_when_already_satisfied`
- `cargo test -p meerkat-core --lib ops_barrier_satisfied_wrong_run_id_rejected`
- `cargo test -p meerkat-core --lib barrier_satisfied_reset_after_tool_calls_resolved`
- `cargo test -p meerkat --lib validate_meerkat_machine_snapshot_reports_turn_ops_barrier_violations`
- `cargo test -p meerkat-runtime --lib`

What a passing review means:

- the authority still owns the pending-op / barrier-op relationship
- the live runner still lowers barrier waiting through the ops lifecycle seam
- the joined Meerkat validator still rejects the malformed K3 states it is
  supposed to reject

## Freeze decision

`K3` is considered frozen for the exact current Meerkat cutover boundary.

What is frozen:

- turn / ops / barrier coupling as currently lowered through the live runner
- the exact safe joined invariants the current snapshot supports

What is explicitly not frozen:

- stronger atomic turn+ops status folklore
- `cancel_after_boundary`
- detached-wake / continuation semantics beyond the exact `K2` asset
