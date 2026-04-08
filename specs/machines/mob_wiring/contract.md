# MobWiringMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-mob` / `generated::mob_wiring`

## State
- Phase enum: `Stable`
- `trusted_operation_peers`: `Set<OperationId>`
- `admitted_peer_input_candidates`: `Set<RawItemId>`
- `admitted_runtime_work_items`: `Set<WorkId>`

## Inputs
- `OperationPeerTrusted`(operation_id: OperationId)
- `PeerInputAdmitted`(raw_item_id: RawItemId, peer_input_class: PeerInputClass)
- `RuntimeWorkAdmitted`(work_id: WorkId, handling_mode: HandlingMode)

## Effects
- `WiringStateUpdated`

## Invariants

## Transitions
### `OperationPeerTrusted`
- From: `Stable`
- On: `OperationPeerTrusted`(operation_id)
- Emits: `WiringStateUpdated`
- To: `Stable`

### `PeerInputAdmitted`
- From: `Stable`
- On: `PeerInputAdmitted`(raw_item_id, peer_input_class)
- Emits: `WiringStateUpdated`
- To: `Stable`

### `RuntimeWorkAdmitted`
- From: `Stable`
- On: `RuntimeWorkAdmitted`(work_id, handling_mode)
- Emits: `WiringStateUpdated`
- To: `Stable`

## Coverage
### Code Anchors
- `meerkat-mob/src/runtime/actor.rs` — peer wiring and admission routes drive canonical wiring state
- `meerkat-mob/src/runtime/roster_authority.rs` — roster/peer graph mutation precursor for canonical wiring state
- `meerkat-mob/src/runtime/edge_locks.rs` — wire/unwire lock discipline precursor tied to canonical wiring ownership

### Scenarios
- `operation-peer-trust-observed` — operation peer-trust events update canonical wiring state
- `peer-input-admission-observed` — peer input candidate admission updates canonical wiring state
- `runtime-work-admission-observed` — runtime work admission updates canonical wiring state
