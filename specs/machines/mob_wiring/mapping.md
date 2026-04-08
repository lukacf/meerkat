# MobWiringMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `MobWiringMachine`

### Code Anchors
- `mob_runtime_actor`: `meerkat-mob/src/runtime/actor.rs` — peer wiring and admission routes drive canonical wiring state
- `mob_roster_authority`: `meerkat-mob/src/runtime/roster_authority.rs` — roster/peer graph mutation precursor for canonical wiring state
- `mob_edge_locks`: `meerkat-mob/src/runtime/edge_locks.rs` — wire/unwire lock discipline precursor tied to canonical wiring ownership

### Scenarios
- `operation-peer-trust-observed` — operation peer-trust events update canonical wiring state
- `peer-input-admission-observed` — peer input candidate admission updates canonical wiring state
- `runtime-work-admission-observed` — runtime work admission updates canonical wiring state

### Transitions
- `OperationPeerTrusted`
  - anchors: `mob_runtime_actor`, `mob_roster_authority`, `mob_edge_locks`
  - scenarios: `operation-peer-trust-observed`
- `PeerInputAdmitted`
  - anchors: `mob_runtime_actor`, `mob_roster_authority`, `mob_edge_locks`
  - scenarios: `operation-peer-trust-observed`
- `RuntimeWorkAdmitted`
  - anchors: `mob_runtime_actor`, `mob_roster_authority`, `mob_edge_locks`
  - scenarios: `operation-peer-trust-observed`

### Effects
- `WiringStateUpdated`
  - anchors: `mob_runtime_actor`, `mob_roster_authority`, `mob_edge_locks`
  - scenarios: `operation-peer-trust-observed`

### Invariants
- `(none)`


<!-- GENERATED_COVERAGE_END -->
