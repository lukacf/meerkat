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
- `RejectSourceSequenceConflict`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
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
- `SettleRefusedDeliveryAtCursor`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ObserveAlreadyRefusedDelivery`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ClassifyNotCommitted`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ClassifyRefused`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ClassifyApplied`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ClassifyAcknowledgedAhead`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ClassifyPending`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ClassifyMixed`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `BindNewDeliveryRecipients`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ObserveBoundDeliveryRecipients`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `SettlePendingDeliveryRecipient`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ObserveSettledDeliveryRecipient`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `FinishSettledDeliveryRecipients`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ObserveFinishedDeliveryRecipients`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)

### Effects
- `DeliveryRecipientsBound`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `DeliveryRecipientSettled`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `DeliveryRecipientsSettled`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `DeliveryStatusMixed`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
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
- `DeliveryRefused`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `CommitRejectedSourceSequenceConflict`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `DeliveryStatusNotCommitted`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `DeliveryStatusApplied`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `DeliveryStatusAcknowledgedAhead`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `DeliveryStatusPending`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `DeliveryStatusRefused`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)

### Invariants
- `delivery_maps_cover_exactly_the_committed_ids`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
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
- `acknowledged_sequences_are_ahead_of_the_cursor`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `acknowledged_sequences_are_committed`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `refused_deliveries_are_committed_and_passed`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `recipient_bindings_belong_to_committed_deliveries`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `enrolled_row_refusal_matches_its_exact_group`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `recipient_settlements_have_bound_targets`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `recipient_bindings_retain_their_outcome_map`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `recipient_groups_pass_only_after_every_recipient_settles`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `recipient_group_outcomes_have_complete_bindings`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `recipient_group_summary_matches_each_retained_outcome`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)


<!-- GENERATED_COVERAGE_END -->
