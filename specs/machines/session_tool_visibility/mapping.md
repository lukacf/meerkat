# SessionToolVisibilityMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `SessionToolVisibilityMachine`

### Code Anchors
- `session_tool_visibility_state`: `meerkat-core/src/session.rs` — canonical durable session-owned tool visibility state
- `tool_scope_projection_bridge`: `meerkat-core/src/tool_scope.rs` — live session/control-plane visibility projection bridge
- `persistent_visibility_mutation`: `meerkat-session/src/persistent.rs` — live-session-first durable mutation and rollback seam
- `catalog_control_dispatcher`: `meerkat-tools/src/control_plane.rs` — search/load control-plane tools over exact catalogs

### Scenarios
- `stage-filter-and-promote` — persistent filter mutations stage first and only become active at the next calling-llm boundary
- `deferred-load-and-promote` — requested deferred tool names accumulate in staged intent and become callable only after boundary promotion
- `dormant-missing-intent-persists` — requested and filtered names remain durable intent even while temporarily absent from the current callable projection

### Transitions
- `StagePersistentFilter`
  - anchors: `session_tool_visibility_state`, `tool_scope_projection_bridge`, `persistent_visibility_mutation`, `catalog_control_dispatcher`
  - scenarios: `stage-filter-and-promote`
- `RequestDeferredTools`
  - anchors: `session_tool_visibility_state`, `tool_scope_projection_bridge`, `persistent_visibility_mutation`, `catalog_control_dispatcher`
  - scenarios: `stage-filter-and-promote`
- `ApplyBoundaryPromote`
  - anchors: `session_tool_visibility_state`, `tool_scope_projection_bridge`, `persistent_visibility_mutation`, `catalog_control_dispatcher`
  - scenarios: `stage-filter-and-promote`
- `ApplyBoundaryNoop`
  - anchors: `session_tool_visibility_state`, `tool_scope_projection_bridge`, `persistent_visibility_mutation`, `catalog_control_dispatcher`
  - scenarios: `stage-filter-and-promote`

### Effects
- `(none)`

### Invariants
- `active_revision_not_ahead_of_staged`
  - anchors: `session_tool_visibility_state`, `tool_scope_projection_bridge`, `persistent_visibility_mutation`, `catalog_control_dispatcher`
  - scenarios: `stage-filter-and-promote`
- `active_requested_names_subset_of_staged`
  - anchors: `session_tool_visibility_state`, `tool_scope_projection_bridge`, `persistent_visibility_mutation`, `catalog_control_dispatcher`
  - scenarios: `stage-filter-and-promote`
- `equal_revision_means_equal_active_and_staged_state`
  - anchors: `session_tool_visibility_state`, `tool_scope_projection_bridge`, `persistent_visibility_mutation`, `catalog_control_dispatcher`
  - scenarios: `stage-filter-and-promote`


<!-- GENERATED_COVERAGE_END -->
