# RuntimeDeliveryMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `RuntimeDeliveryMachine`

### Code Anchors
- `runtime_delivery_authority` (machine `RuntimeDeliveryMachine`): `crates/meerkat-runtime/src/delivery_inbox.rs` — generated runtime delivery identity, sequence, ordered application, and out-of-band acknowledgement authority with mechanical store CAS

### Scenarios
- `runtime_delivery_idempotent_commit` — a stable delivery identity receives one generated sequence and exact replay reuses it
- `runtime_delivery_ordered_application` — generated cursor authority applies each committed delivery exactly once in order
- `runtime_delivery_out_of_band_acknowledgement` — an acknowledgement at the cursor applies the row; one ahead of the cursor is recorded instead of refused, and the cursor later advances over the contiguous acknowledged prefix without re-application, so out-of-order acknowledgement never wedges the queue

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
- `AcknowledgeNextDelivery`
  - anchors: `runtime_delivery_authority`
  - scenarios: `runtime_delivery_out_of_band_acknowledgement`
- `AcknowledgeAheadOfCursor`
  - anchors: `runtime_delivery_authority`
  - scenarios: `runtime_delivery_out_of_band_acknowledgement`
- `ObserveAlreadyAppliedAcknowledgement`
  - anchors: `runtime_delivery_authority`
  - scenarios: `runtime_delivery_out_of_band_acknowledgement`
- `AdvanceOverAcknowledgedDelivery`
  - anchors: `runtime_delivery_authority`
  - scenarios: `runtime_delivery_out_of_band_acknowledgement`
- `AdvanceAcknowledgedPrefixNothingParked`
  - anchors: `runtime_delivery_authority`
  - scenarios: `runtime_delivery_out_of_band_acknowledgement`

### Effects
- `DeliveryCommitted`
  - anchors: `runtime_delivery_authority`
  - scenarios: `runtime_delivery_idempotent_commit`
- `DeliveryReused`
  - anchors: `runtime_delivery_authority`
  - scenarios: `runtime_delivery_idempotent_commit`
- `DeliveryApplied`
  - anchors: `runtime_delivery_authority`
  - scenarios: `runtime_delivery_ordered_application`, `runtime_delivery_out_of_band_acknowledgement`
- `DeliveryAcknowledged`
  - anchors: `runtime_delivery_authority`
  - scenarios: `runtime_delivery_out_of_band_acknowledgement`
- `AcknowledgedPrefixAdvanced`
  - anchors: `runtime_delivery_authority`
  - scenarios: `runtime_delivery_out_of_band_acknowledgement`
- `AcknowledgedPrefixAtRest`
  - anchors: `runtime_delivery_authority`
  - scenarios: `runtime_delivery_out_of_band_acknowledgement`

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
- `applied_cursor_is_never_acknowledged_pending`
  - anchors: `runtime_delivery_authority`
  - scenarios: `runtime_delivery_out_of_band_acknowledgement`


<!-- GENERATED_COVERAGE_END -->
