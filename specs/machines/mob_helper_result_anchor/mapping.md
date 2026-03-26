# MobHelperResultAnchorMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `MobHelperResultAnchorMachine`

### Code Anchors
- `mob_runtime_handle`: `meerkat-mob/src/runtime/handle.rs` — helper-facing surfaces that return run/member result classes
- `mob_runtime_actor`: `meerkat-mob/src/runtime/actor.rs` — runtime command handling path that mirrors terminal classes for helper surfaces
- `mob_terminalization`: `meerkat-mob/src/runtime/terminalization.rs` — run terminalization precursor for helper-result class observations

### Scenarios
- `helper-completed-class-observed` — completed run classes are mirrored into helper-result observation state
- `helper-failed-class-observed` — failed and cancelled run classes are mirrored into helper-result observation state
- `helper-force-cancel-observed` — force-cancel helper classification signals are mirrored into helper-result observation state

### Transitions
- `AnchorCompleted`
  - anchors: `mob_runtime_handle`, `mob_runtime_actor`, `mob_terminalization`
  - scenarios: `helper-completed-class-observed`
- `AnchorFailed`
  - anchors: `mob_runtime_handle`, `mob_runtime_actor`, `mob_terminalization`
  - scenarios: `helper-completed-class-observed`
- `AnchorCancelled`
  - anchors: `mob_runtime_handle`, `mob_runtime_actor`, `mob_terminalization`
  - scenarios: `helper-completed-class-observed`
- `AnchorForceCancelled`
  - anchors: `mob_runtime_handle`, `mob_runtime_actor`, `mob_terminalization`
  - scenarios: `helper-completed-class-observed`

### Effects
- `HelperResultSnapshotUpdated`
  - anchors: `mob_runtime_handle`, `mob_runtime_actor`, `mob_terminalization`
  - scenarios: `helper-completed-class-observed`

### Invariants
- `completed_not_failed`
  - anchors: `mob_runtime_handle`, `mob_runtime_actor`, `mob_terminalization`
  - scenarios: `helper-completed-class-observed`
- `completed_not_cancelled`
  - anchors: `mob_runtime_handle`, `mob_runtime_actor`, `mob_terminalization`
  - scenarios: `helper-completed-class-observed`
- `failed_not_cancelled`
  - anchors: `mob_runtime_handle`, `mob_runtime_actor`, `mob_terminalization`
  - scenarios: `helper-completed-class-observed`


<!-- GENERATED_COVERAGE_END -->
