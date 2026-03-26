# MobRuntimeBridgeAnchorMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `MobRuntimeBridgeAnchorMachine`

### Code Anchors
- `mob_runtime_actor`: `meerkat-mob/src/runtime/actor.rs` — mob actor emits runtime run submission/terminalization and stop-request boundary events
- `mob_provisioner`: `meerkat-mob/src/runtime/provisioner.rs` — runtime session bridge and queue handling precursor for runtime bridge boundaries
- `mob_ops_adapter`: `meerkat-mob/src/runtime/ops_adapter.rs` — ops/runtime linkage precursor for run-level bridge observations

### Scenarios
- `runtime-run-submission-observed` — runtime run submissions are mirrored into runtime-bridge observation state
- `runtime-run-terminal-observed` — runtime run completion/failure/cancellation is mirrored into runtime-bridge observation state
- `runtime-stop-request-observed` — runtime stop requests are mirrored into runtime-bridge observation state

### Transitions
- `RuntimeRunSubmitted`
  - anchors: `mob_runtime_actor`, `mob_provisioner`, `mob_ops_adapter`
  - scenarios: `runtime-run-submission-observed`
- `RuntimeRunCompleted`
  - anchors: `mob_runtime_actor`, `mob_provisioner`, `mob_ops_adapter`
  - scenarios: `runtime-run-submission-observed`
- `RuntimeRunFailed`
  - anchors: `mob_runtime_actor`, `mob_provisioner`, `mob_ops_adapter`
  - scenarios: `runtime-run-submission-observed`
- `RuntimeRunCancelled`
  - anchors: `mob_runtime_actor`, `mob_provisioner`, `mob_ops_adapter`
  - scenarios: `runtime-run-submission-observed`
- `RuntimeStopRequested`
  - anchors: `mob_runtime_actor`, `mob_provisioner`, `mob_ops_adapter`
  - scenarios: `runtime-run-submission-observed`

### Effects
- `RuntimeBridgeSnapshotUpdated`
  - anchors: `mob_runtime_actor`, `mob_provisioner`, `mob_ops_adapter`
  - scenarios: `runtime-run-submission-observed`

### Invariants
- `(none)`


<!-- GENERATED_COVERAGE_END -->
