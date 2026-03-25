# CommsDrainLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-runtime` / `generated::comms_drain_lifecycle`

## State
- Phase enum: `Inactive | Starting | Running | ExitedRespawnable | Stopped`
- `mode`: `Option<CommsDrainMode>`

## Inputs
- `EnsureRunning`(mode: CommsDrainMode)
- `TaskSpawned`
- `TaskExited`(reason: DrainExitReason)
- `StopRequested`
- `AbortObserved`

## Effects
- `SpawnDrainTask`(mode: CommsDrainMode)
- `AbortDrainTask`

## Invariants
- `active_implies_mode_set`

## Transitions
### `EnsureRunningFromInactive`
- From: `Inactive`
- On: `EnsureRunning`(mode)
- Emits: `SpawnDrainTask`
- To: `Starting`

### `TaskSpawnedFromStarting`
- From: `Starting`
- On: `TaskSpawned`()
- To: `Running`

### `TaskExitedFromStartingRespawnable`
- From: `Starting`
- On: `TaskExited`(reason)
- Guards:
  - `reason_is_failed`
  - `mode_is_persistent_host`
- To: `ExitedRespawnable`

### `TaskExitedFromStartingStopped`
- From: `Starting`
- On: `TaskExited`(reason)
- Guards:
  - `not_respawnable`
- To: `Stopped`

### `TaskExitedFromRunningRespawnable`
- From: `Running`
- On: `TaskExited`(reason)
- Guards:
  - `reason_is_failed`
  - `mode_is_persistent_host`
- To: `ExitedRespawnable`

### `TaskExitedFromRunningStopped`
- From: `Running`
- On: `TaskExited`(reason)
- Guards:
  - `not_respawnable`
- To: `Stopped`

### `StopRequestedFromRunning`
- From: `Running`
- On: `StopRequested`()
- Emits: `AbortDrainTask`
- To: `Stopped`

### `StopRequestedFromStarting`
- From: `Starting`
- On: `StopRequested`()
- Emits: `AbortDrainTask`
- To: `Stopped`

### `EnsureRunningFromExitedRespawnable`
- From: `ExitedRespawnable`
- On: `EnsureRunning`(mode)
- Emits: `SpawnDrainTask`
- To: `Starting`

### `StopRequestedFromExitedRespawnable`
- From: `ExitedRespawnable`
- On: `StopRequested`()
- To: `Stopped`

### `EnsureRunningFromStopped`
- From: `Stopped`
- On: `EnsureRunning`(mode)
- Emits: `SpawnDrainTask`
- To: `Starting`

### `AbortObservedFromActive`
- From: `Running`, `Starting`
- On: `AbortObserved`()
- To: `Stopped`

## Coverage
### Code Anchors
- `meerkat-runtime/src/comms_drain_lifecycle_authority.rs` — comms drain lifecycle authority (sealed mutator + evaluate)
- `meerkat-runtime/src/session_adapter.rs` — session adapter comms drain slot wiring and effect execution
- `meerkat-runtime/src/comms_drain.rs` — comms drain task spawn and loop implementation

### Scenarios
- `spawn-run-exit` — drain task spawns, runs, and exits cleanly with suppression lifecycle
- `persistent-respawn` — persistent-host drain respawns after transient failure
- `stop-abort` — drain task is stopped or aborted and suppression is lifted
