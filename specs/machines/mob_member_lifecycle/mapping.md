# MobMemberLifecycleMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `MobMemberLifecycleMachine`

### Code Anchors
- `mob_runtime_actor`: `meerkat-mob/src/runtime/actor.rs` — mob actor observes child operation peer exposure and terminalization routes
- `mob_ops_adapter`: `meerkat-mob/src/runtime/ops_adapter.rs` — runtime/ops bridge that carries operation lifecycle signals
- `mob_provisioner`: `meerkat-mob/src/runtime/provisioner.rs` — member provisioning/retirement bridge where child operation lineage is surfaced

### Scenarios
- `member-peer-exposure-observed` — operation peer-ready exposure is mirrored into member lifecycle observation state
- `member-terminalization-observed` — operation terminalization is mirrored into member lifecycle observation state
- `member-lifecycle-observation-lineage` — member lifecycle owner tracks peer exposure and terminalization routes without overloading bootstrap failure semantics

### Transitions
- `MemberPeerExposed`
  - anchors: `mob_runtime_actor`, `mob_ops_adapter`, `mob_provisioner`
  - scenarios: `member-peer-exposure-observed`
- `MemberTerminalized`
  - anchors: `mob_runtime_actor`, `mob_ops_adapter`, `mob_provisioner`
  - scenarios: `member-peer-exposure-observed`

### Effects
- `MemberLifecycleStateUpdated`
  - anchors: `mob_runtime_actor`, `mob_ops_adapter`, `mob_provisioner`
  - scenarios: `member-peer-exposure-observed`

### Invariants
- `(none)`


<!-- GENERATED_COVERAGE_END -->
