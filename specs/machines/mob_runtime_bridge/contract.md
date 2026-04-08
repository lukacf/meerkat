# MobRuntimeBridgeMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-mob` / `generated::mob_runtime_bridge`

## State
- Phase enum: `Stable`
- `observed_submitted_runs`: `Set<RunId>`
- `observed_completed_runs`: `Set<RunId>`
- `observed_failed_runs`: `Set<RunId>`
- `observed_cancelled_runs`: `Set<RunId>`
- `stop_request_count`: `u32`

## Inputs
- `RuntimeRunSubmitted`(run_id: RunId)
- `RuntimeRunCompleted`(run_id: RunId)
- `RuntimeRunFailed`(run_id: RunId)
- `RuntimeRunCancelled`(run_id: RunId)
- `RuntimeStopRequested`

## Effects
- `RuntimeBridgeStateUpdated`

## Invariants

## Transitions
### `RuntimeRunSubmitted`
- From: `Stable`
- On: `RuntimeRunSubmitted`(run_id)
- Emits: `RuntimeBridgeStateUpdated`
- To: `Stable`

### `RuntimeRunCompleted`
- From: `Stable`
- On: `RuntimeRunCompleted`(run_id)
- Emits: `RuntimeBridgeStateUpdated`
- To: `Stable`

### `RuntimeRunFailed`
- From: `Stable`
- On: `RuntimeRunFailed`(run_id)
- Emits: `RuntimeBridgeStateUpdated`
- To: `Stable`

### `RuntimeRunCancelled`
- From: `Stable`
- On: `RuntimeRunCancelled`(run_id)
- Emits: `RuntimeBridgeStateUpdated`
- To: `Stable`

### `RuntimeStopRequested`
- From: `Stable`
- On: `RuntimeStopRequested`()
- Emits: `RuntimeBridgeStateUpdated`
- To: `Stable`

## Coverage
### Code Anchors
- `meerkat-mob/src/runtime/actor.rs` — mob actor emits runtime run submission/terminalization and stop-request boundary events
- `meerkat-mob/src/runtime/provisioner.rs` — runtime session bridge and queue handling precursor for runtime bridge boundaries
- `meerkat-mob/src/runtime/ops_adapter.rs` — ops/runtime linkage precursor for run-level bridge observations

### Scenarios
- `runtime-run-submission-observed` — runtime run submissions update canonical runtime-bridge state
- `runtime-run-terminal-observed` — runtime run completion/failure/cancellation update canonical runtime-bridge state
- `runtime-stop-request-observed` — runtime stop requests update canonical runtime-bridge state
