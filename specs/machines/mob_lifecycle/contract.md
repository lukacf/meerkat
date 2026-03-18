# MobLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `2`
- Rust owner: `meerkat-mob` / `generated::mob_lifecycle`

## State
- Phase enum: `Creating | Running | Stopped | Completed | Destroyed`
- `active_run_count`: `u32`
- `cleanup_pending`: `Bool`

## Inputs
- `Start`
- `Stop`
- `Resume`
- `MarkCompleted`
- `Destroy`
- `StartRun`
- `FinishRun`
- `BeginCleanup`
- `FinishCleanup`

## Effects
- `EmitLifecycleNotice`
- `RequestCleanup`

## Invariants
- `destroyed_has_no_active_runs`
- `completed_has_no_active_runs`

## Transitions
### `Start`
- From: `Creating`, `Stopped`
- On: `Start`()
- To: `Running`

### `Stop`
- From: `Running`
- On: `Stop`()
- To: `Stopped`

### `Resume`
- From: `Stopped`
- On: `Resume`()
- To: `Running`

### `MarkCompleted`
- From: `Running`, `Stopped`
- On: `MarkCompleted`()
- Guards:
  - `no_active_runs`
- To: `Completed`

### `Destroy`
- From: `Creating`, `Running`, `Stopped`, `Completed`
- On: `Destroy`()
- Emits: `EmitLifecycleNotice`
- To: `Destroyed`

### `StartRun`
- From: `Running`
- On: `StartRun`()
- To: `Running`

### `FinishRun`
- From: `Running`, `Stopped`
- On: `FinishRun`()
- Guards:
  - `has_active_runs`
- To: `Running`

### `BeginCleanup`
- From: `Stopped`, `Completed`
- On: `BeginCleanup`()
- Emits: `RequestCleanup`
- To: `Stopped`

### `FinishCleanup`
- From: `Stopped`, `Completed`
- On: `FinishCleanup`()
- To: `Stopped`

## Coverage
### Code Anchors
- `meerkat-mob/src/runtime/state.rs` — mob lifecycle state precursor
- `meerkat-mob/src/runtime/actor.rs` — serialized lifecycle owner precursor
- `meerkat-mob/src/runtime/handle.rs` — public lifecycle handle precursor

### Scenarios
- `start-stop-resume` — mob lifecycle transitions through start/stop/resume cleanly
- `run-count-and-cleanup` — active run count and cleanup semantics stay consistent
- `complete-destroy` — completed/destroyed lifecycle phases stay terminal
