# CommsDrainLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-runtime` / `generated::comms_drain_lifecycle`

## State
- Phase enum: `Inactive | Starting | Running | ExitedRespawnable | Stopped`
- `mode`: `Option<CommsDrainMode>`
- `suppresses_turn_boundary_drain`: `Bool`

## Inputs
- `EnsureRunning`(mode: CommsDrainMode)
- `TaskSpawned`
- `TaskExited`(reason: DrainExitReason)
- `StopRequested`
- `AbortObserved`

## Effects
- `SpawnDrainTask`(mode: CommsDrainMode)
- `SetTurnBoundaryDrainSuppressed`(active: Bool)
- `AbortDrainTask`

## Invariants
- `active_implies_mode_set`
- `active_implies_suppression`

## Transitions
### `EnsureRunningFromInactive`
- From: `Inactive`
- On: `EnsureRunning`(mode)
- Emits: `SpawnDrainTask`, `SetTurnBoundaryDrainSuppressed`
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
- Emits: `SetTurnBoundaryDrainSuppressed`
- To: `ExitedRespawnable`

### `TaskExitedFromStartingStopped`
- From: `Starting`
- On: `TaskExited`(reason)
- Guards:
  - `not_respawnable`
- Emits: `SetTurnBoundaryDrainSuppressed`
- To: `Stopped`

### `TaskExitedFromRunningRespawnable`
- From: `Running`
- On: `TaskExited`(reason)
- Guards:
  - `reason_is_failed`
  - `mode_is_persistent_host`
- Emits: `SetTurnBoundaryDrainSuppressed`
- To: `ExitedRespawnable`

### `TaskExitedFromRunningStopped`
- From: `Running`
- On: `TaskExited`(reason)
- Guards:
  - `not_respawnable`
- Emits: `SetTurnBoundaryDrainSuppressed`
- To: `Stopped`

### `StopRequestedFromRunning`
- From: `Running`
- On: `StopRequested`()
- Emits: `AbortDrainTask`, `SetTurnBoundaryDrainSuppressed`
- To: `Stopped`

### `StopRequestedFromStarting`
- From: `Starting`
- On: `StopRequested`()
- Emits: `AbortDrainTask`, `SetTurnBoundaryDrainSuppressed`
- To: `Stopped`

### `EnsureRunningFromExitedRespawnable`
- From: `ExitedRespawnable`
- On: `EnsureRunning`(mode)
- Emits: `SpawnDrainTask`, `SetTurnBoundaryDrainSuppressed`
- To: `Starting`

### `StopRequestedFromExitedRespawnable`
- From: `ExitedRespawnable`
- On: `StopRequested`()
- To: `Stopped`

### `EnsureRunningFromStopped`
- From: `Stopped`
- On: `EnsureRunning`(mode)
- Emits: `SpawnDrainTask`, `SetTurnBoundaryDrainSuppressed`
- To: `Starting`

### `AbortObservedFromActive`
- From: `Running`, `Starting`
- On: `AbortObserved`()
- Emits: `SetTurnBoundaryDrainSuppressed`
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
