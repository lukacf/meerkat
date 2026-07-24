# RuntimeDeliveryMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `RuntimeDeliveryMachine`

### Code Anchors
- `runtime_delivery_authority` (machine `RuntimeDeliveryMachine`): `meerkat-runtime/src/delivery_inbox.rs` — generated runtime delivery identity, sequence, and ordered application authority with mechanical store CAS

### Scenarios
- `runtime_delivery_idempotent_commit` — a stable delivery identity receives one generated sequence and exact replay reuses it
- `runtime_delivery_ordered_application` — generated cursor authority applies each committed delivery exactly once in order

### Transitions
- `CommitNewDelivery`
  - anchors: `runtime_delivery_authority`
  - scenarios: `runtime_delivery_idempotent_commit`
- `ReuseCommittedDelivery`
  - anchors: `runtime_delivery_authority`
  - scenarios: `runtime_delivery_idempotent_commit`
- `ApplyNextDelivery`
  - anchors: `runtime_delivery_authority`
  - scenarios: `runtime_delivery_ordered_application`
- `ObserveAlreadyAppliedDelivery`
  - anchors: `runtime_delivery_authority`
  - scenarios: `runtime_delivery_ordered_application`

### Effects
- `DeliveryCommitted`
  - anchors: `runtime_delivery_authority`
  - scenarios: `runtime_delivery_idempotent_commit`
- `DeliveryReused`
  - anchors: `runtime_delivery_authority`
  - scenarios: `runtime_delivery_idempotent_commit`
- `DeliveryApplied`
  - anchors: `runtime_delivery_authority`
  - scenarios: `runtime_delivery_ordered_application`

### Invariants
- `applied_cursor_does_not_pass_committed_sequence`
  - anchors: `runtime_delivery_authority`
  - scenarios: (unclaimed)
- `empty_delivery_set_has_zero_sequence`
  - anchors: `runtime_delivery_authority`
  - scenarios: (unclaimed)
- `delivery_identity_and_sequence_cardinality_match`
  - anchors: `runtime_delivery_authority`
  - scenarios: (unclaimed)
- `committed_sequence_cardinality_tracks_high_water`
  - anchors: `runtime_delivery_authority`
  - scenarios: (unclaimed)


<!-- GENERATED_COVERAGE_END -->
