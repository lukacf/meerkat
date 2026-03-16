# ExternalToolSurfaceMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-mcp` / `machines::external_tool_surface`

## State
- Phase enum: `Operating | Shutdown`
- `known_surfaces`: `Set<SurfaceId>`
- `visible_surfaces`: `Set<SurfaceId>`
- `base_state`: `Map<SurfaceId, SurfaceBaseState>`
- `pending_op`: `Map<SurfaceId, PendingSurfaceOp>`
- `staged_op`: `Map<SurfaceId, StagedSurfaceOp>`
- `inflight_calls`: `Map<SurfaceId, u64>`
- `last_delta_operation`: `Map<SurfaceId, SurfaceDeltaOperation>`
- `last_delta_phase`: `Map<SurfaceId, SurfaceDeltaPhase>`

## Inputs
- `StageAdd`(surface_id: SurfaceId)
- `StageRemove`(surface_id: SurfaceId)
- `StageReload`(surface_id: SurfaceId)
- `ApplyBoundary`(surface_id: SurfaceId, applied_at_turn: TurnNumber)
- `PendingSucceeded`(surface_id: SurfaceId, applied_at_turn: TurnNumber)
- `PendingFailed`(surface_id: SurfaceId, applied_at_turn: TurnNumber)
- `CallStarted`(surface_id: SurfaceId)
- `CallFinished`(surface_id: SurfaceId)
- `FinalizeRemovalClean`(surface_id: SurfaceId, applied_at_turn: TurnNumber)
- `FinalizeRemovalForced`(surface_id: SurfaceId, applied_at_turn: TurnNumber)
- `Shutdown`

## Effects
- `ScheduleSurfaceCompletion`(surface_id: SurfaceId, operation: SurfaceDeltaOperation)
- `RefreshVisibleSurfaceSet`
- `EmitExternalToolDelta`(surface_id: SurfaceId, operation: SurfaceDeltaOperation, phase: SurfaceDeltaPhase, persisted: Bool, applied_at_turn: TurnNumber)
- `CloseSurfaceConnection`(surface_id: SurfaceId)
- `RejectSurfaceCall`(surface_id: SurfaceId, reason: String)

## Helpers
- `SurfaceBase`(surface_id: SurfaceId) -> `SurfaceBaseState`
- `PendingOp`(surface_id: SurfaceId) -> `PendingSurfaceOp`
- `StagedOp`(surface_id: SurfaceId) -> `StagedSurfaceOp`
- `InflightCallCount`(surface_id: SurfaceId) -> `u64`
- `LastDeltaOperation`(surface_id: SurfaceId) -> `SurfaceDeltaOperation`
- `LastDeltaPhase`(surface_id: SurfaceId) -> `SurfaceDeltaPhase`
- `IsVisible`(surface_id: SurfaceId) -> `Bool`

## Invariants
- `removing_or_removed_surfaces_are_not_visible`
- `visible_membership_matches_active_base_state`
- `removing_surfaces_have_no_pending_add_or_reload`
- `removed_surfaces_only_allow_pending_none_or_add`
- `inflight_calls_only_exist_for_active_or_removing_surfaces`
- `reload_pending_requires_active_base_state`
- `removed_surfaces_have_zero_inflight_calls`
- `forced_delta_phase_is_always_a_remove_delta`

## Transitions
### `StageAdd`
- From: `Operating`
- On: `StageAdd`(surface_id)
- To: `Operating`

### `StageRemove`
- From: `Operating`
- On: `StageRemove`(surface_id)
- To: `Operating`

### `StageReload`
- From: `Operating`
- On: `StageReload`(surface_id)
- Guards:
  - `surface_is_active`
- To: `Operating`

### `ApplyBoundaryAdd`
- From: `Operating`
- On: `ApplyBoundary`(surface_id, applied_at_turn)
- Guards:
  - `staged_add_present`
  - `base_state_accepts_add`
- Emits: `ScheduleSurfaceCompletion`, `EmitExternalToolDelta`
- To: `Operating`

### `ApplyBoundaryReload`
- From: `Operating`
- On: `ApplyBoundary`(surface_id, applied_at_turn)
- Guards:
  - `staged_reload_present`
  - `reload_requires_active_base`
- Emits: `ScheduleSurfaceCompletion`, `EmitExternalToolDelta`
- To: `Operating`

### `ApplyBoundaryRemoveDraining`
- From: `Operating`
- On: `ApplyBoundary`(surface_id, applied_at_turn)
- Guards:
  - `staged_remove_present`
  - `remove_begins_from_active`
- Emits: `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Operating`

### `ApplyBoundaryRemoveNoop`
- From: `Operating`
- On: `ApplyBoundary`(surface_id, applied_at_turn)
- Guards:
  - `staged_remove_present`
  - `remove_not_starting_from_active`
- To: `Operating`

### `PendingSucceededAdd`
- From: `Operating`
- On: `PendingSucceeded`(surface_id, applied_at_turn)
- Guards:
  - `add_is_pending`
- Emits: `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Operating`

### `PendingSucceededReload`
- From: `Operating`
- On: `PendingSucceeded`(surface_id, applied_at_turn)
- Guards:
  - `reload_is_pending`
- Emits: `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Operating`

### `PendingFailedAdd`
- From: `Operating`
- On: `PendingFailed`(surface_id, applied_at_turn)
- Guards:
  - `add_is_pending`
- Emits: `EmitExternalToolDelta`
- To: `Operating`

### `PendingFailedReload`
- From: `Operating`
- On: `PendingFailed`(surface_id, applied_at_turn)
- Guards:
  - `reload_is_pending`
- Emits: `EmitExternalToolDelta`
- To: `Operating`

### `CallStartedActive`
- From: `Operating`
- On: `CallStarted`(surface_id)
- Guards:
  - `surface_is_active`
- To: `Operating`

### `CallStartedRejectWhileRemoving`
- From: `Operating`
- On: `CallStarted`(surface_id)
- Guards:
  - `surface_is_removing`
- Emits: `RejectSurfaceCall`
- To: `Operating`

### `CallStartedRejectWhileUnavailable`
- From: `Operating`
- On: `CallStarted`(surface_id)
- Guards:
  - `surface_is_not_dispatchable`
- Emits: `RejectSurfaceCall`
- To: `Operating`

### `CallFinishedActive`
- From: `Operating`
- On: `CallFinished`(surface_id)
- Guards:
  - `surface_is_active`
  - `has_inflight_calls`
- To: `Operating`

### `CallFinishedRemoving`
- From: `Operating`
- On: `CallFinished`(surface_id)
- Guards:
  - `surface_is_removing`
  - `has_inflight_calls`
- To: `Operating`

### `FinalizeRemovalClean`
- From: `Operating`
- On: `FinalizeRemovalClean`(surface_id, applied_at_turn)
- Guards:
  - `surface_is_removing`
  - `no_inflight_calls_remain`
- Emits: `CloseSurfaceConnection`, `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Operating`

### `FinalizeRemovalForced`
- From: `Operating`
- On: `FinalizeRemovalForced`(surface_id, applied_at_turn)
- Guards:
  - `surface_is_removing`
- Emits: `CloseSurfaceConnection`, `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Operating`

### `Shutdown`
- From: `Operating`, `Shutdown`
- On: `Shutdown`()
- To: `Shutdown`

## Coverage
### Code Anchors
- `meerkat-mcp/src/router.rs` — staged MCP surface lifecycle precursor
- `meerkat-mcp/src/adapter.rs` — runtime-facing tool-surface adapter precursor
- `meerkat-core/src/agent/state.rs` — turn-boundary external-tool update consumer precursor

### Scenarios
- `add-reload-remove` — surface add, reload, and removal produce canonical typed deltas
- `draining-removal` — removing surfaces drain inflight work before final removal
- `runtime-scoped-browser-tools` — browser-local tools remain runtime-scoped external surfaces
