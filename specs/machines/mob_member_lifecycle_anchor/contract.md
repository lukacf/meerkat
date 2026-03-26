# MobMemberLifecycleAnchorMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-mob` / `generated::mob_member_lifecycle_anchor`

## State
- Phase enum: `Tracking`
- `observed_peer_exposed_operations`: `Set<OperationId>`
- `observed_terminalized_operations`: `Set<OperationId>`
- `peer_exposure_count`: `u32`
- `terminalization_count`: `u32`

## Inputs
- `MemberPeerExposed`(operation_id: OperationId)
- `MemberTerminalized`(operation_id: OperationId, terminal_outcome: OperationTerminalOutcome)

## Effects
- `MemberLifecycleSnapshotUpdated`

## Invariants

## Transitions
### `MemberPeerExposed`
- From: `Tracking`
- On: `MemberPeerExposed`(operation_id)
- Emits: `MemberLifecycleSnapshotUpdated`
- To: `Tracking`

### `MemberTerminalized`
- From: `Tracking`
- On: `MemberTerminalized`(operation_id, terminal_outcome)
- Emits: `MemberLifecycleSnapshotUpdated`
- To: `Tracking`

## Coverage
### Code Anchors
- `meerkat-mob/src/runtime/actor.rs` — mob actor observes child operation peer exposure and terminalization routes
- `meerkat-mob/src/runtime/ops_adapter.rs` — runtime/ops bridge that carries operation lifecycle signals
- `meerkat-mob/src/runtime/provisioner.rs` — member provisioning/retirement bridge where child operation lineage is surfaced

### Scenarios
- `member-peer-exposure-observed` — operation peer-ready exposure is mirrored into member lifecycle observation state
- `member-terminalization-observed` — operation terminalization is mirrored into member lifecycle observation state
- `member-lifecycle-observation-lineage` — member lifecycle anchor tracks observation lineage counters and sets without owning canonical lifecycle truth
