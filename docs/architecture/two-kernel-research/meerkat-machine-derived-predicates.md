# MeerkatMachine Derived Predicates

Status: frozen target-state predicate handoff

This note defines the canonical derived predicates used by the target
`MeerkatMachine` freeze package.

Use it with:

- `meerkat-machine-freeze.md`
- `meerkat-machine-state-schema.md`
- `meerkat-machine-transition-catalog.md`
- `meerkat-machine-proof-obligations.md`

## Purpose

Some of the most important machine facts are not primitive fields. They are
derived predicates over the canonical machine state.

This note freezes those predicates so the TLA+ model does not invent them ad
hoc.

## Core Predicates

### `HasLiveBinding`

```text
HasLiveBinding ==
  binding.live = TRUE
```

### `HasActiveRun`

```text
HasActiveRun ==
  control.current_run_id # None
```

### `RunIdsAligned`

```text
RunIdsAligned ==
  /\ control.current_run_id = inputs.current_run_id
  /\ control.current_run_id = turn.active_run_id
```

### `HasQueuedWork`

```text
HasQueuedWork ==
  \/ Len(inputs.queue) > 0
  \/ Len(inputs.steer_queue) > 0
```

### `HasWaitingCompletions`

```text
HasWaitingCompletions ==
  completion.waiting_inputs # {}
```

### `HasPendingOps`

```text
HasPendingOps ==
  turn.pending_op_refs # {}
```

### `BarrierPending`

```text
BarrierPending ==
  /\ turn.phase = WaitingForOps
  /\ turn.barrier_operation_ids # {}
  /\ ~turn.barrier_satisfied
```

### `BarrierReadyToResolve`

```text
BarrierReadyToResolve ==
  /\ BarrierPending
  /\ turn.barrier_operation_ids \subseteq
     { op \in ops.known_operations :
         ops.operation_status[op] \in {Some(Succeeded), Some(Failed), Some(Cancelled)} }
```

### `HasReplayablePeerBacklog`

```text
HasReplayablePeerBacklog ==
  peer.replayable # {}
```

### `HasVisibleToolSurfaces`

```text
HasVisibleToolSurfaces ==
  tools.visible_surfaces # {}
```

### `DrainTaskRequired`

```text
DrainTaskRequired ==
  /\ HasLiveBinding
  /\ drain.mode # None
  /\ drain.mode # Some(Disabled)
```

## Lifecycle-Specific Predicates

### `SettledRetired`

```text
SettledRetired ==
  /\ control.phase = Retired
  /\ control.current_run_id = None
  /\ inputs.current_run_id = None
  /\ turn.active_run_id = None
  /\ completion.waiting_inputs = {}
```

### `SettledStopped`

```text
SettledStopped ==
  /\ control.phase = Stopped
  /\ control.current_run_id = None
  /\ inputs.current_run_id = None
  /\ turn.active_run_id = None
  /\ Len(inputs.queue) = 0
  /\ Len(inputs.steer_queue) = 0
```

### `SettledDestroyed`

```text
SettledDestroyed ==
  /\ control.phase = Destroyed
  /\ HasLiveBinding = FALSE
  /\ Len(inputs.queue) = 0
  /\ Len(inputs.steer_queue) = 0
  /\ completion.waiting_inputs = {}
  /\ drain.phase = Inactive
```

### `ResetDurableState`

```text
ResetDurableState ==
  /\ peer.trusted_peers
  /\ tools.visible_surfaces
```

The target reset semantics preserve only explicitly durable machine-owned
regions. This predicate names that durable subset.

## Execution Predicates

### `BoundaryDraining`

```text
BoundaryDraining ==
  turn.phase = DrainingBoundary
```

### `CancellationPendingAtBoundary`

```text
CancellationPendingAtBoundary ==
  /\ BoundaryDraining
  /\ turn.cancel_after_boundary = TRUE
```

### `Quiescent`

```text
Quiescent ==
  /\ ~HasActiveRun
  /\ ~BoundaryDraining
  /\ control.pending_control = {}
```

This is the minimum target quiescence predicate. A TLA+ model may strengthen it
if a proof requires a stricter machine-owned notion, but not weaken it.

## Acceptance

This predicate set is complete enough when:

- every term used by the target freeze and proof package has a canonical
  machine-level predicate when it is more than a raw field check
- lifecycle “settled” language is grounded in explicit predicates
- quiescence and barrier readiness are explicit predicates

That state is now reached.
