# SessionToolVisibilityMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `SessionToolVisibilityMachine`

### Code Anchors
- `tool_visibility_state`: `meerkat-core/src/tool_scope.rs` — tool visibility projection layered on durable visibility state

### Scenarios
- `staged_visibility_apply` — tool visibility staged state applies and publishes committed revisions at a boundary

### Transitions
- `StagePersistentFilter`
  - anchors: `tool_visibility_state`
  - scenarios: `staged_visibility_apply`
- `RequestDeferredTools`
  - anchors: `tool_visibility_state`
  - scenarios: `staged_visibility_apply`
- `ApplyBoundaryPromote`
  - anchors: `tool_visibility_state`
  - scenarios: `staged_visibility_apply`
- `ApplyBoundaryNoop`
  - anchors: `tool_visibility_state`
  - scenarios: `staged_visibility_apply`

### Effects
- `(none)`

### Invariants
- `active_revision_not_ahead_of_staged`
  - anchors: `tool_visibility_state`
  - scenarios: `staged_visibility_apply`
- `active_requested_names_subset_of_staged`
  - anchors: `tool_visibility_state`
  - scenarios: `staged_visibility_apply`
- `equal_revision_means_equal_active_and_staged_state`
  - anchors: `tool_visibility_state`
  - scenarios: `staged_visibility_apply`


<!-- GENERATED_COVERAGE_END -->
