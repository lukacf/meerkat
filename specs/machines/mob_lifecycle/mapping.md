# MobLifecycleMachine Mapping Note

This note maps the normative `0.5` `MobLifecycleMachine` contract onto current
`0.4` anchors.

## Rust anchors

- lifecycle enum and command algebra:
  - `meerkat-mob/src/runtime/state.rs`
- serialized owner:
  - `meerkat-mob/src/runtime/actor.rs`
- public surface:
  - `meerkat-mob/src/runtime/handle.rs`

## What is already aligned

- `MobState` already exists with `Creating`, `Running`, `Stopped`,
  `Completed`, `Destroyed`
- lifecycle commands are already serialized by `MobActor`
- stop/resume/complete/destroy/reset already exist as first-class commands
- flow trackers already exist as actor-owned lifecycle-adjacent resources

## What the formal model abstracts

The TLA+ model deliberately abstracts away:

- roster membership details
- task-board contents
- wiring graphs
- per-run flow engine internals
- MCP server identity/detail
- surface reply channels

Those refine the same lifecycle owner but are not themselves the lifecycle
contract.

## Important `0.5` clarification

The model keeps `MobLifecycleMachine` intentionally narrow.

That is by design:

- lifecycle transitions stay machine-shaped
- roster/wiring becomes `MobTopologyService`
- task mutation becomes `MobTaskBoardService`
- flow orchestration becomes `FlowRunMachine`

## Known `0.4` divergence

- `MobActor` still centralizes multiple concerns that `0.5` splits apart
- some cleanup/cancellation behavior is currently intertwined with actor logic
  rather than expressed as a standalone lifecycle owner

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `MobLifecycleMachine`

### Code Anchors
- `mob_lifecycle_state`: `meerkat-mob/src/runtime/state.rs` — mob lifecycle state precursor
- `mob_actor`: `meerkat-mob/src/runtime/actor.rs` — serialized lifecycle owner precursor
- `mob_handle`: `meerkat-mob/src/runtime/handle.rs` — public lifecycle handle precursor

### Scenarios
- `start-stop-resume` — mob lifecycle transitions through start/stop/resume cleanly
- `run-count-and-cleanup` — active run count and cleanup semantics stay consistent
- `complete-destroy` — completed/destroyed lifecycle phases stay terminal

### Transitions
- `Start`
  - anchors: `mob_lifecycle_state`, `mob_actor`, `mob_handle`
  - scenarios: `start-stop-resume`
- `Stop`
  - anchors: `mob_lifecycle_state`, `mob_actor`, `mob_handle`
  - scenarios: `start-stop-resume`
- `Resume`
  - anchors: `mob_lifecycle_state`, `mob_actor`, `mob_handle`
  - scenarios: `start-stop-resume`
- `MarkCompleted`
  - anchors: `mob_lifecycle_state`, `mob_actor`, `mob_handle`
  - scenarios: `complete-destroy`
- `Destroy`
  - anchors: `mob_lifecycle_state`, `mob_actor`, `mob_handle`
  - scenarios: `start-stop-resume`
- `StartRun`
  - anchors: `mob_lifecycle_state`, `mob_actor`, `mob_handle`
  - scenarios: `start-stop-resume`
- `FinishRun`
  - anchors: `mob_lifecycle_state`, `mob_actor`, `mob_handle`
  - scenarios: `start-stop-resume`
- `BeginCleanup`
  - anchors: `mob_lifecycle_state`, `mob_actor`, `mob_handle`
  - scenarios: `start-stop-resume`
- `FinishCleanup`
  - anchors: `mob_lifecycle_state`, `mob_actor`, `mob_handle`
  - scenarios: `start-stop-resume`

### Effects
- `EmitLifecycleNotice`
  - anchors: `mob_lifecycle_state`, `mob_actor`, `mob_handle`
  - scenarios: `start-stop-resume`
- `RequestCleanup`
  - anchors: `mob_lifecycle_state`, `mob_actor`, `mob_handle`
  - scenarios: `start-stop-resume`

### Invariants
- `destroyed_has_no_active_runs`
  - anchors: `mob_lifecycle_state`, `mob_actor`, `mob_handle`
  - scenarios: `start-stop-resume`
- `completed_has_no_active_runs`
  - anchors: `mob_lifecycle_state`, `mob_actor`, `mob_handle`
  - scenarios: `complete-destroy`


<!-- GENERATED_COVERAGE_END -->
