# CommsDrainLifecycleMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `CommsDrainLifecycleMachine`

### Code Anchors
- `comms_drain_authority`: `meerkat-runtime/src/comms_drain_lifecycle_authority.rs` — comms drain lifecycle authority (sealed mutator + evaluate)
- `session_adapter_drain`: `meerkat-runtime/src/session_adapter.rs` — session adapter comms drain slot wiring and effect execution
- `comms_drain_spawn`: `meerkat-runtime/src/comms_drain.rs` — comms drain task spawn and loop implementation

### Scenarios
- `spawn-run-exit` — drain task spawns, runs, and exits cleanly
- `persistent-respawn` — persistent-host drain respawns after transient failure
- `stop-abort` — drain task is stopped or aborted

### Transitions
- `EnsureRunningFromInactive`
  - anchors: `comms_drain_authority`, `session_adapter_drain`, `comms_drain_spawn`
  - scenarios: `spawn-run-exit`, `persistent-respawn`
- `TaskSpawnedFromStarting`
  - anchors: `comms_drain_authority`, `session_adapter_drain`, `comms_drain_spawn`
  - scenarios: `spawn-run-exit`, `persistent-respawn`
- `TaskExitedFromStartingRespawnable`
  - anchors: `comms_drain_authority`, `session_adapter_drain`, `comms_drain_spawn`
  - scenarios: `persistent-respawn`
- `TaskExitedFromStartingStopped`
  - anchors: `comms_drain_authority`, `session_adapter_drain`, `comms_drain_spawn`
  - scenarios: `stop-abort`
- `TaskExitedFromRunningRespawnable`
  - anchors: `comms_drain_authority`, `session_adapter_drain`, `comms_drain_spawn`
  - scenarios: `persistent-respawn`
- `TaskExitedFromRunningStopped`
  - anchors: `comms_drain_authority`, `session_adapter_drain`, `comms_drain_spawn`
  - scenarios: `stop-abort`
- `StopRequestedFromRunning`
  - anchors: `comms_drain_authority`, `session_adapter_drain`, `comms_drain_spawn`
  - scenarios: `stop-abort`
- `StopRequestedFromStarting`
  - anchors: `comms_drain_authority`, `session_adapter_drain`, `comms_drain_spawn`
  - scenarios: `stop-abort`
- `EnsureRunningFromExitedRespawnable`
  - anchors: `comms_drain_authority`, `session_adapter_drain`, `comms_drain_spawn`
  - scenarios: `persistent-respawn`
- `StopRequestedFromExitedRespawnable`
  - anchors: `comms_drain_authority`, `session_adapter_drain`, `comms_drain_spawn`
  - scenarios: `stop-abort`
- `EnsureRunningFromStopped`
  - anchors: `comms_drain_authority`, `session_adapter_drain`, `comms_drain_spawn`
  - scenarios: `stop-abort`
- `AbortObservedFromActive`
  - anchors: `comms_drain_authority`, `session_adapter_drain`, `comms_drain_spawn`
  - scenarios: `stop-abort`

### Effects
- `SpawnDrainTask`
  - anchors: `comms_drain_authority`, `session_adapter_drain`, `comms_drain_spawn`
  - scenarios: `spawn-run-exit`, `persistent-respawn`
- `AbortDrainTask`
  - anchors: `comms_drain_authority`, `session_adapter_drain`, `comms_drain_spawn`
  - scenarios: `stop-abort`

### Invariants
- `active_implies_mode_set`
  - anchors: `comms_drain_authority`, `session_adapter_drain`, `comms_drain_spawn`
  - scenarios: `spawn-run-exit`, `persistent-respawn`


<!-- GENERATED_COVERAGE_END -->
