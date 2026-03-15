# ExternalToolSurfaceMachine

Status: normative `0.5` machine contract

## Purpose

`ExternalToolSurfaceMachine` owns the live lifecycle of dynamically attached
tool surfaces for one runtime instance.

For the MCP-backed `0.4` implementation anchor, this is the concern currently
spread across `McpRouter`, `McpRouterAdapter`, and the runtime-facing boundary
update path.

It is the authoritative owner of:

- staged add/remove/reload intents
- per-surface activation/removal lifecycle
- pending async activation/reload bookkeeping
- inflight-call-aware removal draining
- visible tool-surface membership
- one canonical outward typed tool-surface delta contract

It is **not** the owner of:

- peer comms
- ordinary runtime ingress
- transcript projection of updates
- tool visibility policy beyond live surface membership
- LLM/tool-call execution scheduling outside surface-local dispatch guards

## Scope Boundary

This machine begins after a surface-level tool operation has already been
normalized into a tool-surface command such as:

- add this surface
- reload this surface
- remove this surface

It ends at a typed `ExternalToolDelta` plus the derived visible-surface set. If
runtime chooses to render an update into transcript-visible synthetic context,
that is a projection owned elsewhere.

## Authoritative State Model

For one runtime instance, the machine state is the tuple:

- `base_state: Map<SurfaceId, SurfaceBaseState>`
- `pending_op: Map<SurfaceId, Option<PendingSurfaceOp>>`
- `staged_op: Map<SurfaceId, Option<StagedSurfaceOp>>`
- `inflight_calls: Map<SurfaceId, u32>`
- `last_delta_operation: Map<SurfaceId, SurfaceDeltaOperation>`
- `last_delta_phase: Map<SurfaceId, SurfaceDeltaPhase>`

`SurfaceBaseState` is:

- `Absent`
- `Active`
- `Removing`
- `Removed`

`PendingSurfaceOp` is:

- `Add`
- `Reload`

`StagedSurfaceOp` is:

- `Add`
- `Remove`
- `Reload`

`SurfaceDeltaOperation` is:

- `None`
- `Add`
- `Remove`
- `Reload`

`SurfaceDeltaPhase` is:

- `None`
- `Pending`
- `Applied`
- `Draining`
- `Forced`
- `Failed`

Important semantic rules:

- visible tool membership is derived from `base_state = Active`
- pending add/reload does not make a surface visible by itself
- `Removing` is not visible, even though addressability metadata may still
  exist long enough to reject new calls cleanly
- the outward contract is one canonical typed delta:
  `ExternalToolDelta { target, operation, phase, persisted, applied_at_turn }`

## Input Alphabet

The closed external input/command alphabet for this machine is:

- `StageAdd(surface_id)`
- `StageRemove(surface_id)`
- `StageReload(surface_id)`
- `ApplyBoundary(surface_id)`
- `PendingSucceeded(surface_id)`
- `PendingFailed(surface_id)`
- `CallStarted(surface_id)`
- `CallFinished(surface_id)`
- `FinalizeRemovalClean(surface_id)`
- `FinalizeRemovalForced(surface_id)`
- `Shutdown`

Notes:

- `ApplyBoundary` applies one already-staged operation for one surface. The
  real implementation may batch boundary apply; the formal machine models one
  normalized transition at a time.
- `PendingSucceeded` / `PendingFailed` are completion inputs from background
  activation work.
- removal finalization is separate from removal start because the machine must
  respect inflight drain/timeout semantics.

## Effect Family

The closed machine-boundary effect family is:

- `ScheduleSurfaceCompletion(surface_id, operation)`
- `RefreshVisibleSurfaceSet`
- `EmitExternalToolDelta(surface_id, operation, phase, persisted, applied_at_turn)`
- `CloseSurfaceConnection(surface_id)`
- `RejectSurfaceCall(surface_id, reason)`

These are architecture-level effect names. `McpLifecycleAction`,
`ExternalToolNotice`, transcript/system notices, and transport-specific update
objects are all projections or shell realizations of this one canonical delta
contract.

## Transition Relation

### Staging

1. `StageAdd`

State updates:

- `staged_op[surface_id] := Add`

2. `StageRemove`

State updates:

- `staged_op[surface_id] := Remove`

3. `StageReload`

Preconditions:

- `base_state[surface_id] = Active`

State updates:

- `staged_op[surface_id] := Reload`

### Boundary apply

4. `ApplyBoundary(surface_id)` with staged add

Preconditions:

- `staged_op[surface_id] = Add`
- `base_state[surface_id] ∈ {Absent, Active, Removed}`

State updates:

- `pending_op[surface_id] := Add`
- `staged_op[surface_id] := None`
- `last_delta_operation[surface_id] := Add`
- `last_delta_phase[surface_id] := Pending`

Effects:

- `ScheduleSurfaceCompletion(surface_id, Add)`
- `EmitExternalToolDelta(surface_id, Add, Pending, persisted = false, applied_at_turn = current)`

5. `ApplyBoundary(surface_id)` with staged reload

Preconditions:

- `staged_op[surface_id] = Reload`
- `base_state[surface_id] = Active`

State updates:

- `pending_op[surface_id] := Reload`
- `staged_op[surface_id] := None`
- `last_delta_operation[surface_id] := Reload`
- `last_delta_phase[surface_id] := Pending`

Important semantic rule:

- reload preserves current visibility while pending

Effects:

- `ScheduleSurfaceCompletion(surface_id, Reload)`
- `EmitExternalToolDelta(surface_id, Reload, Pending, persisted = false, applied_at_turn = current)`

6. `ApplyBoundary(surface_id)` with staged remove

State updates:

- `staged_op[surface_id] := None`
- `pending_op[surface_id] := None`
- if `base_state[surface_id] = Active`, then
  `base_state[surface_id] := Removing`
- if removal begins:
  - `last_delta_operation[surface_id] := Remove`
  - `last_delta_phase[surface_id] := Draining`

Important semantic rules:

- remove cancels any pending add/reload for the same surface
- once `Removing`, the surface is no longer visible

### Pending completion

7. `PendingSucceeded`

Preconditions:

- `pending_op[surface_id] ∈ {Add, Reload}`

State updates:

- `pending_op[surface_id] := None`
- `base_state[surface_id] := Active`
- `last_delta_operation[surface_id] := pending_op_pre_state`
- `last_delta_phase[surface_id] := Applied`

Effects:

- `RefreshVisibleSurfaceSet`
- `EmitExternalToolDelta(surface_id, pending_op_pre_state, Applied, persisted = true, applied_at_turn = current)`

8. `PendingFailed`

Preconditions:

- `pending_op[surface_id] ∈ {Add, Reload}`

State updates:

- `pending_op[surface_id] := None`
- `base_state[surface_id]` is preserved
- `last_delta_operation[surface_id] := pending_op_pre_state`
- `last_delta_phase[surface_id] := Failed`

Important semantic rule:

- add failure leaves the previous base state intact
- reload failure preserves the previously active surface

### Dispatch and removal drain

9. `CallStarted`

Preconditions:

- `base_state[surface_id] = Active`

State updates:

- `inflight_calls[surface_id] += 1`

10. `CallFinished`

Preconditions:

- `base_state[surface_id] ∈ {Active, Removing}`
- `inflight_calls[surface_id] > 0`

State updates:

- `inflight_calls[surface_id] -= 1`

11. `FinalizeRemovalClean`

Preconditions:

- `base_state[surface_id] = Removing`
- `inflight_calls[surface_id] = 0`

State updates:

- `base_state[surface_id] := Removed`
- `last_delta_operation[surface_id] := Remove`
- `last_delta_phase[surface_id] := Applied`

Effects:

- `CloseSurfaceConnection(surface_id)`
- `RefreshVisibleSurfaceSet`
- `EmitExternalToolDelta(surface_id, Remove, Applied, persisted = true, applied_at_turn = current)`

12. `FinalizeRemovalForced`

Preconditions:

- `base_state[surface_id] = Removing`

State updates:

- `base_state[surface_id] := Removed`
- `inflight_calls[surface_id] := 0`
- `last_delta_operation[surface_id] := Remove`
- `last_delta_phase[surface_id] := Forced`

Effects:

- `CloseSurfaceConnection(surface_id)`
- `RefreshVisibleSurfaceSet`
- `EmitExternalToolDelta(surface_id, Remove, Forced, persisted = true, applied_at_turn = current)`

### Shutdown

13. `Shutdown`

State updates:

- every surface becomes `Absent`
- every `pending_op := None`
- every `staged_op := None`
- every `inflight_calls := 0`
- every `last_delta_operation := None`
- every `last_delta_phase := None`

## Invariants

The machine must preserve:

1. `Removing` and `Removed` surfaces are not visible
2. pending add/reload does not by itself make a surface visible
3. `Removing` surfaces may not have a pending add/reload
4. `Removed` surfaces may only have `pending_op ∈ {None, Add}`
5. `inflight_calls > 0` is only allowed in `Active` or `Removing`
6. reload may only be pending while base state remains `Active`
7. `Removed` surfaces have zero inflight calls
8. forced removal is explicit and distinguishable in the typed delta contract

## Implementation Anchors

Current anchors include:

- `meerkat-mcp/src/router.rs`
- `meerkat-mcp/src/adapter.rs`
- runtime/tool-boundary update consumption in
  `meerkat-core/src/agent/state.rs`

Those anchors already embody most of the target machine behavior; `0.5` is
primarily formalization and convergence onto one outward delta contract, not
invention from scratch.
