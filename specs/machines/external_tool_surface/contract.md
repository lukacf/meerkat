# ExternalToolSurfaceMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `2`
- Rust owner: `meerkat-mcp` / `generated::external_tool_surface`

## State
- Phase enum: `Operating | Shutdown`
- `known_surfaces`: `Set<SurfaceId>`
- `visible_surfaces`: `Set<SurfaceId>`
- `base_state`: `Map<SurfaceId, SurfaceBaseState>`
- `pending_op`: `Map<SurfaceId, PendingSurfaceOp>`
- `staged_op`: `Map<SurfaceId, StagedSurfaceOp>`
- `staged_intent_sequence`: `Map<SurfaceId, u64>`
- `next_staged_intent_sequence`: `u64`
- `pending_task_sequence`: `Map<SurfaceId, u64>`
- `pending_lineage_sequence`: `Map<SurfaceId, u64>`
- `next_pending_task_sequence`: `u64`
- `inflight_calls`: `Map<SurfaceId, u64>`
- `last_delta_operation`: `Map<SurfaceId, SurfaceDeltaOperation>`
- `last_delta_phase`: `Map<SurfaceId, SurfaceDeltaPhase>`
- `snapshot_epoch`: `u64`
- `snapshot_aligned_epoch`: `u64`

## Inputs
- `StageAdd`(surface_id: SurfaceId)
- `StageRemove`(surface_id: SurfaceId)
- `StageReload`(surface_id: SurfaceId)
- `ApplyBoundary`(surface_id: SurfaceId, applied_at_turn: TurnNumber)
- `PendingSucceeded`(surface_id: SurfaceId, operation: SurfaceDeltaOperation, pending_task_sequence: u64, staged_intent_sequence: u64, applied_at_turn: TurnNumber)
- `PendingFailed`(surface_id: SurfaceId, operation: SurfaceDeltaOperation, pending_task_sequence: u64, staged_intent_sequence: u64, applied_at_turn: TurnNumber)
- `CallStarted`(surface_id: SurfaceId)
- `CallFinished`(surface_id: SurfaceId)
- `FinalizeRemovalClean`(surface_id: SurfaceId, applied_at_turn: TurnNumber)
- `FinalizeRemovalForced`(surface_id: SurfaceId, applied_at_turn: TurnNumber)
- `SnapshotAligned`(snapshot_epoch: u64)
- `Shutdown`

## Effects
- `ScheduleSurfaceCompletion`(surface_id: SurfaceId, operation: SurfaceDeltaOperation, pending_task_sequence: u64, staged_intent_sequence: u64, applied_at_turn: TurnNumber)
- `RefreshVisibleSurfaceSet`(snapshot_epoch: u64)
- `EmitExternalToolDelta`(surface_id: SurfaceId, operation: SurfaceDeltaOperation, phase: SurfaceDeltaPhase, persisted: Bool, applied_at_turn: TurnNumber)
- `CloseSurfaceConnection`(surface_id: SurfaceId)
- `RejectSurfaceCall`(surface_id: SurfaceId, reason: String)

## Helpers
- `SurfaceBase`(surface_id: SurfaceId) -> `SurfaceBaseState`
- `PendingOp`(surface_id: SurfaceId) -> `PendingSurfaceOp`
- `StagedOp`(surface_id: SurfaceId) -> `StagedSurfaceOp`
- `StagedIntentSequence`(surface_id: SurfaceId) -> `u64`
- `PendingTaskSequence`(surface_id: SurfaceId) -> `u64`
- `PendingLineageSequence`(surface_id: SurfaceId) -> `u64`
- `InflightCallCount`(surface_id: SurfaceId) -> `u64`
- `LastDeltaOperation`(surface_id: SurfaceId) -> `SurfaceDeltaOperation`
- `LastDeltaPhase`(surface_id: SurfaceId) -> `SurfaceDeltaPhase`
- `IsVisible`(surface_id: SurfaceId) -> `Bool`

## Invariants
- `visible_surfaces_subset_of_known_surfaces`
- `base_state_keys_subset_of_known_surfaces`
- `pending_op_keys_subset_of_known_surfaces`
- `staged_op_keys_subset_of_known_surfaces`
- `staged_intent_sequence_keys_subset_of_known_surfaces`
- `pending_task_sequence_keys_subset_of_known_surfaces`
- `pending_lineage_sequence_keys_subset_of_known_surfaces`
- `inflight_calls_keys_subset_of_known_surfaces`
- `last_delta_operation_keys_subset_of_known_surfaces`
- `last_delta_phase_keys_subset_of_known_surfaces`
- `removing_or_removed_surfaces_are_not_visible`
- `visible_membership_matches_active_base_state`
- `removing_surfaces_have_no_pending_add_or_reload`
- `removed_surfaces_only_allow_pending_none_or_add`
- `inflight_calls_only_exist_for_active_or_removing_surfaces`
- `reload_pending_requires_active_base_state`
- `removed_surfaces_have_zero_inflight_calls`
- `forced_delta_phase_is_always_a_remove_delta`
- `staged_sequence_matches_staged_presence`
- `pending_lineage_matches_pending_presence`
- `snapshot_alignment_epoch_not_ahead`

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
  - `no_pending_operation`
  - `base_state_accepts_add`
- Emits: `ScheduleSurfaceCompletion`, `EmitExternalToolDelta`
- To: `Operating`

### `ApplyBoundaryReload`
- From: `Operating`
- On: `ApplyBoundary`(surface_id, applied_at_turn)
- Guards:
  - `staged_reload_present`
  - `no_pending_operation`
  - `reload_requires_active_base`
- Emits: `ScheduleSurfaceCompletion`, `EmitExternalToolDelta`
- To: `Operating`

### `ApplyBoundaryRemoveDraining`
- From: `Operating`
- On: `ApplyBoundary`(surface_id, applied_at_turn)
- Guards:
  - `staged_remove_present`
  - `no_pending_operation`
  - `remove_begins_from_active`
- Emits: `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Operating`

### `ApplyBoundaryRemoveNoop`
- From: `Operating`
- On: `ApplyBoundary`(surface_id, applied_at_turn)
- Guards:
  - `staged_remove_present`
  - `no_pending_operation`
  - `remove_not_starting_from_active`
- To: `Operating`

### `PendingSucceededAdd`
- From: `Operating`
- On: `PendingSucceeded`(surface_id, operation, pending_task_sequence, staged_intent_sequence, applied_at_turn)
- Guards:
  - `operation_is_add`
  - `pending_operation_matches`
  - `pending_task_sequence_matches`
  - `pending_lineage_sequence_matches`
- Emits: `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Operating`

### `PendingSucceededReload`
- From: `Operating`
- On: `PendingSucceeded`(surface_id, operation, pending_task_sequence, staged_intent_sequence, applied_at_turn)
- Guards:
  - `operation_is_reload`
  - `pending_operation_matches`
  - `pending_task_sequence_matches`
  - `pending_lineage_sequence_matches`
- Emits: `RefreshVisibleSurfaceSet`, `EmitExternalToolDelta`
- To: `Operating`

### `PendingFailedAdd`
- From: `Operating`
- On: `PendingFailed`(surface_id, operation, pending_task_sequence, staged_intent_sequence, applied_at_turn)
- Guards:
  - `operation_is_add`
  - `pending_operation_matches`
  - `pending_task_sequence_matches`
  - `pending_lineage_sequence_matches`
- Emits: `EmitExternalToolDelta`
- To: `Operating`

### `PendingFailedReload`
- From: `Operating`
- On: `PendingFailed`(surface_id, operation, pending_task_sequence, staged_intent_sequence, applied_at_turn)
- Guards:
  - `operation_is_reload`
  - `pending_operation_matches`
  - `pending_task_sequence_matches`
  - `pending_lineage_sequence_matches`
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

### `SnapshotAligned`
- From: `Operating`
- On: `SnapshotAligned`(snapshot_epoch)
- Guards:
  - `snapshot_epoch_matches_current`
  - `snapshot_alignment_was_pending`
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
