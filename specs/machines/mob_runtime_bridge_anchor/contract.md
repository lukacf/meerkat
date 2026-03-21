# MobRuntimeBridgeAnchorMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-mob` / `generated::mob_runtime_bridge_anchor`

## State
- Phase enum: `Tracking`
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
- `RuntimeBridgeSnapshotUpdated`

## Invariants

## Transitions
### `RuntimeRunSubmitted`
- From: `Tracking`
- On: `RuntimeRunSubmitted`(run_id)
- Emits: `RuntimeBridgeSnapshotUpdated`
- To: `Tracking`

### `RuntimeRunCompleted`
- From: `Tracking`
- On: `RuntimeRunCompleted`(run_id)
- Emits: `RuntimeBridgeSnapshotUpdated`
- To: `Tracking`

### `RuntimeRunFailed`
- From: `Tracking`
- On: `RuntimeRunFailed`(run_id)
- Emits: `RuntimeBridgeSnapshotUpdated`
- To: `Tracking`

### `RuntimeRunCancelled`
- From: `Tracking`
- On: `RuntimeRunCancelled`(run_id)
- Emits: `RuntimeBridgeSnapshotUpdated`
- To: `Tracking`

### `RuntimeStopRequested`
- From: `Tracking`
- On: `RuntimeStopRequested`()
- Emits: `RuntimeBridgeSnapshotUpdated`
- To: `Tracking`

## Coverage
### Code Anchors
- `meerkat-mob/src/runtime/actor.rs` — mob actor emits runtime run submission/terminalization and stop-request boundary events
- `meerkat-mob/src/runtime/provisioner.rs` — runtime session bridge and queue handling precursor for runtime bridge boundaries
- `meerkat-mob/src/runtime/ops_adapter.rs` — ops/runtime linkage precursor for run-level bridge observations

### Scenarios
- `runtime-run-submission-observed` — runtime run submissions are mirrored into runtime-bridge observation state
- `runtime-run-terminal-observed` — runtime run completion/failure/cancellation is mirrored into runtime-bridge observation state
- `runtime-stop-request-observed` — runtime stop requests are mirrored into runtime-bridge observation state
