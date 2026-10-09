# RuntimeDeliveryMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::runtime_delivery`

## State
- Phase enum: `Active`
- `delivery_ids`: `Set<String>`
- `delivery_sequences`: `Map<String, u64>`
- `delivery_source_sequences`: `Map<String, u64>`
- `committed_sequences`: `Set<u64>`
- `next_sequence`: `u64`
- `applied_cursor`: `u64`
- `acknowledged_sequences`: `Set<u64>`
- `refused_deliveries`: `Map<String, DeliveryRefusalReason>`
- `delivery_recipient_bindings`: `Map<String, Map<String, String>>`
- `recipient_outcomes`: `Map<String, Map<String, DeliveryRecipientOutcome>>`
- `recipient_group_outcomes`: `Map<String, DeliveryRecipientGroupOutcome>`

## Inputs
- `BindDeliveryRecipients`(delivery_id: String, delivery_sequence: u64, recipients: Map<String, String>)
- `SettleDeliveryRecipient`(delivery_id: String, delivery_sequence: u64, recipient_id: String, target_binding: String, outcome: DeliveryRecipientOutcome)
- `FinishDeliveryRecipients`(delivery_id: String, delivery_sequence: u64)
- `CommitDelivery`(delivery_id: String, source_sequence: u64)
- `MarkDeliveryApplied`(delivery_id: String, delivery_sequence: u64)
- `AcknowledgeDelivery`(delivery_id: String, delivery_sequence: u64)
- `AdvanceAcknowledgedPrefix`
- `SettleRefusedDelivery`(delivery_id: String, delivery_sequence: u64, reason: DeliveryRefusalReason)
- `ClassifyDeliveryStatus`(delivery_id: String)

## Signals

## Effects
- `DeliveryRecipientsBound`(delivery_id: String, delivery_sequence: u64)
- `DeliveryRecipientSettled`(delivery_id: String, delivery_sequence: u64, recipient_id: String, outcome: DeliveryRecipientOutcome)
- `DeliveryRecipientsSettled`(delivery_id: String, delivery_sequence: u64, outcome: DeliveryRecipientGroupOutcome)
- `DeliveryStatusMixed`(delivery_id: String, delivery_sequence: u64, bindings: Map<String, String>, outcomes: Map<String, DeliveryRecipientOutcome>)
- `DeliveryCommitted`(delivery_id: String, source_sequence: u64, delivery_sequence: u64)
- `DeliveryReused`(delivery_id: String, source_sequence: u64, delivery_sequence: u64)
- `DeliveryApplied`(delivery_id: String, delivery_sequence: u64)
- `DeliveryAcknowledged`(delivery_id: String, delivery_sequence: u64)
- `AcknowledgedPrefixAdvanced`(delivery_sequence: u64)
- `AcknowledgedPrefixAtRest`(applied_cursor: u64)
- `DeliveryRefused`(delivery_id: String, delivery_sequence: u64, reason: DeliveryRefusalReason)
- `CommitRejectedSourceSequenceConflict`(delivery_id: String, source_sequence: u64, committed_source_sequence: u64)
- `DeliveryStatusNotCommitted`(delivery_id: String)
- `DeliveryStatusApplied`(delivery_id: String, delivery_sequence: u64)
- `DeliveryStatusAcknowledgedAhead`(delivery_id: String, delivery_sequence: u64)
- `DeliveryStatusPending`(delivery_id: String, delivery_sequence: u64)
- `DeliveryStatusRefused`(delivery_id: String, delivery_sequence: u64, reason: DeliveryRefusalReason)

## Invariants
- `delivery_maps_cover_exactly_the_committed_ids`
- `applied_cursor_does_not_pass_committed_sequence`
- `empty_delivery_set_has_zero_sequence`
- `delivery_identity_and_sequence_cardinality_match`
- `committed_sequence_cardinality_tracks_high_water`
- `applied_cursor_is_never_acknowledged_pending`
- `acknowledged_sequences_are_ahead_of_the_cursor`
- `acknowledged_sequences_are_committed`
- `refused_deliveries_are_committed_and_passed`
- `recipient_bindings_belong_to_committed_deliveries`
- `enrolled_row_refusal_matches_its_exact_group`
- `recipient_settlements_have_bound_targets`
- `recipient_bindings_retain_their_outcome_map`
- `recipient_groups_pass_only_after_every_recipient_settles`
- `recipient_group_outcomes_have_complete_bindings`
- `recipient_group_summary_matches_each_retained_outcome`

## Transitions
### `CommitNewDelivery`
- From: `Active`
- On: `CommitDelivery`(delivery_id, source_sequence)
- Guards:
  - ``
- Emits: `DeliveryCommitted`
- To: `Active`

### `ReuseCommittedDelivery`
- From: `Active`
- On: `CommitDelivery`(delivery_id, source_sequence)
- Guards:
  - ``
- Emits: `DeliveryReused`
- To: `Active`

### `RejectSourceSequenceConflict`
- From: `Active`
- On: `CommitDelivery`(delivery_id, source_sequence)
- Guards:
  - ``
- Emits: `CommitRejectedSourceSequenceConflict`
- To: `Active`

### `ApplyNextDelivery`
- From: `Active`
- On: `MarkDeliveryApplied`(delivery_id, delivery_sequence)
- Guards:
  - ``
- Emits: `DeliveryApplied`
- To: `Active`

### `ObserveAlreadyAppliedDelivery`
- From: `Active`
- On: `MarkDeliveryApplied`(delivery_id, delivery_sequence)
- Guards:
  - ``
- Emits: `DeliveryApplied`
- To: `Active`

### `AcknowledgeNextDelivery`
- From: `Active`
- On: `AcknowledgeDelivery`(delivery_id, delivery_sequence)
- Guards:
  - ``
- Emits: `DeliveryApplied`
- To: `Active`

### `AcknowledgeAheadOfCursor`
- From: `Active`
- On: `AcknowledgeDelivery`(delivery_id, delivery_sequence)
- Guards:
  - ``
- Emits: `DeliveryAcknowledged`
- To: `Active`

### `ObserveAlreadyAppliedAcknowledgement`
- From: `Active`
- On: `AcknowledgeDelivery`(delivery_id, delivery_sequence)
- Guards:
  - ``
- Emits: `DeliveryApplied`
- To: `Active`

### `AdvanceOverAcknowledgedDelivery`
- From: `Active`
- On: `AdvanceAcknowledgedPrefix`()
- Guards:
  - ``
- Emits: `AcknowledgedPrefixAdvanced`
- To: `Active`

### `AdvanceAcknowledgedPrefixNothingParked`
- From: `Active`
- On: `AdvanceAcknowledgedPrefix`()
- Guards:
  - ``
- Emits: `AcknowledgedPrefixAtRest`
- To: `Active`

### `SettleRefusedDeliveryAtCursor`
- From: `Active`
- On: `SettleRefusedDelivery`(delivery_id, delivery_sequence, reason)
- Guards:
  - ``
- Emits: `DeliveryRefused`
- To: `Active`

### `ObserveAlreadyRefusedDelivery`
- From: `Active`
- On: `SettleRefusedDelivery`(delivery_id, delivery_sequence, reason)
- Guards:
  - ``
- Emits: `DeliveryRefused`
- To: `Active`

### `ClassifyNotCommitted`
- From: `Active`
- On: `ClassifyDeliveryStatus`(delivery_id)
- Guards:
  - ``
- Emits: `DeliveryStatusNotCommitted`
- To: `Active`

### `ClassifyRefused`
- From: `Active`
- On: `ClassifyDeliveryStatus`(delivery_id)
- Guards:
  - ``
- Emits: `DeliveryStatusRefused`
- To: `Active`

### `ClassifyApplied`
- From: `Active`
- On: `ClassifyDeliveryStatus`(delivery_id)
- Guards:
  - ``
- Emits: `DeliveryStatusApplied`
- To: `Active`

### `ClassifyAcknowledgedAhead`
- From: `Active`
- On: `ClassifyDeliveryStatus`(delivery_id)
- Guards:
  - ``
- Emits: `DeliveryStatusAcknowledgedAhead`
- To: `Active`

### `ClassifyPending`
- From: `Active`
- On: `ClassifyDeliveryStatus`(delivery_id)
- Guards:
  - ``
- Emits: `DeliveryStatusPending`
- To: `Active`

### `ClassifyMixed`
- From: `Active`
- On: `ClassifyDeliveryStatus`(delivery_id)
- Guards:
  - ``
- Emits: `DeliveryStatusMixed`
- To: `Active`

### `BindNewDeliveryRecipients`
- From: `Active`
- On: `BindDeliveryRecipients`(delivery_id, delivery_sequence, recipients)
- Guards:
  - ``
- Emits: `DeliveryRecipientsBound`
- To: `Active`

### `ObserveBoundDeliveryRecipients`
- From: `Active`
- On: `BindDeliveryRecipients`(delivery_id, delivery_sequence, recipients)
- Guards:
  - ``
- Emits: `DeliveryRecipientsBound`
- To: `Active`

### `SettlePendingDeliveryRecipient`
- From: `Active`
- On: `SettleDeliveryRecipient`(delivery_id, delivery_sequence, recipient_id, target_binding, outcome)
- Guards:
  - ``
- Emits: `DeliveryRecipientSettled`
- To: `Active`

### `ObserveSettledDeliveryRecipient`
- From: `Active`
- On: `SettleDeliveryRecipient`(delivery_id, delivery_sequence, recipient_id, target_binding, outcome)
- Guards:
  - ``
- Emits: `DeliveryRecipientSettled`
- To: `Active`

### `FinishSettledDeliveryRecipients`
- From: `Active`
- On: `FinishDeliveryRecipients`(delivery_id, delivery_sequence)
- Guards:
  - ``
- Emits: `DeliveryRecipientsSettled`
- To: `Active`

### `ObserveFinishedDeliveryRecipients`
- From: `Active`
- On: `FinishDeliveryRecipients`(delivery_id, delivery_sequence)
- Guards:
  - ``
- Emits: `DeliveryRecipientsSettled`
- To: `Active`

## Coverage
### Code Anchors
- `runtime_delivery_authority` (machine `RuntimeDeliveryMachine`): `crates/meerkat-runtime/src/delivery_inbox.rs` — generated runtime delivery identity, sequence, ordered application, and out-of-band acknowledgement authority with mechanical store CAS

### Scenarios
- `runtime_delivery_idempotent_commit` — a stable delivery identity receives one generated sequence and exact replay reuses it
- `runtime_delivery_ordered_application` — generated cursor authority applies each committed delivery exactly once in order
- `runtime_delivery_out_of_band_acknowledgement` — an acknowledgement at the cursor applies the row; one ahead of the cursor is recorded instead of refused, and the cursor later advances over the contiguous acknowledged prefix without re-application, so out-of-order acknowledgement never wedges the queue
