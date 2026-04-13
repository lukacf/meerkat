# SessionToolVisibilityMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-core` / `generated::session_tool_visibility`

## State
- Phase enum: `Operating`
- `inherited_base_filter`: `ToolFilter`
- `active_filter`: `ToolFilter`
- `staged_filter`: `ToolFilter`
- `active_requested_deferred_names`: `Set<String>`
- `staged_requested_deferred_names`: `Set<String>`
- `requested_witnesses`: `Map<String, ToolVisibilityWitness>`
- `filter_witnesses`: `Map<String, ToolVisibilityWitness>`
- `active_revision`: `u64`
- `staged_revision`: `u64`

## Inputs
- `StagePersistentFilter`(filter: ToolFilter, witnesses: Map<String, ToolVisibilityWitness>)
- `RequestDeferredTools`(names: Set<String>, witnesses: Map<String, ToolVisibilityWitness>)
- `ApplyBoundary`

## Effects

## Helpers
- `HasPendingPromotion`() -> `Bool`
- `RequestedWitnessKeys`() -> `Set<String>`
- `FilterWitnessKeys`() -> `Set<String>`

## Invariants
- `active_revision_not_ahead_of_staged`
- `active_requested_names_subset_of_staged`
- `equal_revision_means_equal_active_and_staged_state`

## Transitions
### `StagePersistentFilter`
- From: `Operating`
- On: `StagePersistentFilter`(filter, witnesses)
- To: `Operating`

### `RequestDeferredTools`
- From: `Operating`
- On: `RequestDeferredTools`(names, witnesses)
- To: `Operating`

### `ApplyBoundaryPromote`
- From: `Operating`
- On: `ApplyBoundary`()
- Guards:
  - `has_pending_promotion`
- To: `Operating`

### `ApplyBoundaryNoop`
- From: `Operating`
- On: `ApplyBoundary`()
- Guards:
  - `no_pending_promotion`
- To: `Operating`

## Coverage
### Code Anchors
- `meerkat-core/src/tool_scope.rs` — tool visibility projection layered on durable visibility state

### Scenarios
- `staged_visibility_apply` — tool visibility staged state applies and publishes committed revisions at a boundary
