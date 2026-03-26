# MobWiringAnchorMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-mob` / `generated::mob_wiring_anchor`

## State
- Phase enum: `Tracking`
- `observed_trusted_operation_peers`: `Set<OperationId>`
- `observed_peer_input_candidates`: `Set<RawItemId>`
- `observed_runtime_work_items`: `Set<WorkId>`

## Inputs
- `OperationPeerTrusted`(operation_id: OperationId)
- `PeerInputAdmitted`(raw_item_id: RawItemId, peer_input_class: PeerInputClass)
- `RuntimeWorkAdmitted`(work_id: WorkId, handling_mode: HandlingMode)

## Effects
- `WiringSnapshotUpdated`

## Invariants

## Transitions
### `OperationPeerTrusted`
- From: `Tracking`
- On: `OperationPeerTrusted`(operation_id)
- Emits: `WiringSnapshotUpdated`
- To: `Tracking`

### `PeerInputAdmitted`
- From: `Tracking`
- On: `PeerInputAdmitted`(raw_item_id, peer_input_class)
- Emits: `WiringSnapshotUpdated`
- To: `Tracking`

### `RuntimeWorkAdmitted`
- From: `Tracking`
- On: `RuntimeWorkAdmitted`(work_id, handling_mode)
- Emits: `WiringSnapshotUpdated`
- To: `Tracking`

## Coverage
### Code Anchors
- `meerkat-mob/src/runtime/actor.rs` — peer wiring and admission routes mirrored into wiring observation state
- `meerkat-mob/src/runtime/roster_authority.rs` — roster/peer graph mutation precursor for wiring boundary observations
- `meerkat-mob/src/runtime/edge_locks.rs` — wire/unwire lock discipline precursor tied to observed wiring boundaries

### Scenarios
- `operation-peer-trust-observed` — operation peer-trust events are mirrored into wiring observation state
- `peer-input-admission-observed` — peer input candidate admission is mirrored into wiring observation state
- `runtime-work-admission-observed` — runtime work admission is mirrored into wiring observation state
