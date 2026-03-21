# MobHelperResultAnchorMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-mob` / `generated::mob_helper_result_anchor`

## State
- Phase enum: `Tracking`
- `observed_completed_runs`: `Set<RunId>`
- `observed_failed_runs`: `Set<RunId>`
- `observed_cancelled_runs`: `Set<RunId>`
- `force_cancel_count`: `u32`

## Inputs
- `AnchorCompleted`(run_id: RunId)
- `AnchorFailed`(run_id: RunId)
- `AnchorCancelled`(run_id: RunId)
- `AnchorForceCancelled`

## Effects
- `HelperResultSnapshotUpdated`

## Invariants
- `completed_not_failed`
- `completed_not_cancelled`
- `failed_not_cancelled`

## Transitions
### `AnchorCompleted`
- From: `Tracking`
- On: `AnchorCompleted`(run_id)
- Emits: `HelperResultSnapshotUpdated`
- To: `Tracking`

### `AnchorFailed`
- From: `Tracking`
- On: `AnchorFailed`(run_id)
- Emits: `HelperResultSnapshotUpdated`
- To: `Tracking`

### `AnchorCancelled`
- From: `Tracking`
- On: `AnchorCancelled`(run_id)
- Emits: `HelperResultSnapshotUpdated`
- To: `Tracking`

### `AnchorForceCancelled`
- From: `Tracking`
- On: `AnchorForceCancelled`()
- Emits: `HelperResultSnapshotUpdated`
- To: `Tracking`

## Coverage
### Code Anchors
- `meerkat-mob/src/runtime/handle.rs` — helper-facing surfaces that return run/member result classes
- `meerkat-mob/src/runtime/actor.rs` — runtime command handling path that mirrors terminal classes for helper surfaces
- `meerkat-mob/src/runtime/terminalization.rs` — run terminalization precursor for helper-result class observations

### Scenarios
- `helper-completed-class-observed` — completed run classes are mirrored into helper-result observation state
- `helper-failed-class-observed` — failed and cancelled run classes are mirrored into helper-result observation state
- `helper-force-cancel-observed` — force-cancel helper classification signals are mirrored into helper-result observation state
