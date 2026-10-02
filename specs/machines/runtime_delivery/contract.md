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

## Inputs
- `CommitDelivery`(delivery_id: String, source_sequence: u64)
- `MarkDeliveryApplied`(delivery_id: String, delivery_sequence: u64)
- `AcknowledgeDelivery`(delivery_id: String, delivery_sequence: u64)
- `AdvanceAcknowledgedPrefix`

## Signals

## Effects
- `DeliveryCommitted`(delivery_id: String, source_sequence: u64, delivery_sequence: u64)
- `DeliveryReused`(delivery_id: String, source_sequence: u64, delivery_sequence: u64)
- `DeliveryApplied`(delivery_id: String, delivery_sequence: u64)
- `DeliveryAcknowledged`(delivery_id: String, delivery_sequence: u64)
- `AcknowledgedPrefixAdvanced`(delivery_sequence: u64)

## Invariants
- `applied_cursor_does_not_pass_committed_sequence`
- `empty_delivery_set_has_zero_sequence`
- `delivery_identity_and_sequence_cardinality_match`
- `committed_sequence_cardinality_tracks_high_water`
- `applied_cursor_is_never_acknowledged_pending`

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

## Coverage
### Code Anchors
- `runtime_delivery_authority` (machine `RuntimeDeliveryMachine`): `crates/meerkat-runtime/src/delivery_inbox.rs` — generated runtime delivery identity, sequence, ordered application, and out-of-band acknowledgement authority with mechanical store CAS

### Scenarios
- `runtime_delivery_idempotent_commit` — a stable delivery identity receives one generated sequence and exact replay reuses it
- `runtime_delivery_ordered_application` — generated cursor authority applies each committed delivery exactly once in order
- `runtime_delivery_out_of_band_acknowledgement` — an acknowledgement at the cursor applies the row; one ahead of the cursor is recorded instead of refused, and the cursor later advances over the contiguous acknowledged prefix without re-application, so out-of-order acknowledgement never wedges the queue
