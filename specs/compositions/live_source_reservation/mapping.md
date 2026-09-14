# live_source_reservation Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `live_source_reservation`

### Code Anchors
- `live_source_joint_store_commit` (route `selected_range_freezes_source`): `meerkat-runtime/src/live_ledger/authority/source_reservation.rs` — Native selected-range producer binds exact composite context and both generated candidates to one source/head CAS; public provider installation and formal qualification remain separate.

### Scenarios
- `source-selected-range-transaction` — Native source tests cover empty and paged exact content, delayed admission, source-first replay and actual SQLite reopen; this is not public provider qualification.

### Routes
- `selected_range_freezes_source`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ingress_close_fences_request_sources`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)

### Scheduler Rules
- `(none)`

### Invariants
- `selected_frontier_and_source_publish_together`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)


<!-- GENERATED_COVERAGE_END -->
