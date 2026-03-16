# runtime_pipeline Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `runtime_pipeline`

### Code Anchors
- `runtime_loop`: `meerkat-runtime/src/runtime_loop.rs` — runtime orchestration precursor for control/ingress/execution
- `core_executor`: `meerkat-core/src/lifecycle/core_executor.rs` — turn execution bridge precursor
- `run_event`: `meerkat-core/src/lifecycle/run_event.rs` — boundary/completion effect surface precursor

### Scenarios
- `runtime-success-path` — staged work begins a run, applies a boundary, and completes
- `runtime-failure-rollback` — failed run rolls staged contributors back before steady state
- `runtime-cancel-rollback` — cancelled run rolls staged contributors back before steady state
- `control-preemption` — control-plane work preempts ordinary ingress scheduling

### Routes
- `staged_run_notifies_control`
  - anchors: `runtime_loop`, `core_executor`, `run_event`
  - scenarios: `runtime-success-path`
- `control_starts_execution`
  - anchors: `runtime_loop`, `core_executor`, `run_event`
  - scenarios: `runtime-success-path`
- `execution_boundary_updates_ingress`
  - anchors: `runtime_loop`, `core_executor`, `run_event`
  - scenarios: `runtime-success-path`
- `execution_completion_updates_ingress`
  - anchors: `runtime_loop`, `core_executor`, `run_event`
  - scenarios: `runtime-success-path`
- `execution_completion_notifies_control`
  - anchors: `runtime_loop`, `core_executor`, `run_event`
  - scenarios: `runtime-success-path`
- `execution_failure_updates_ingress`
  - anchors: `runtime_loop`, `core_executor`, `run_event`
  - scenarios: `runtime-failure-rollback`
- `execution_failure_notifies_control`
  - anchors: `runtime_loop`, `core_executor`, `run_event`
  - scenarios: `runtime-failure-rollback`
- `execution_cancel_updates_ingress`
  - anchors: `runtime_loop`, `core_executor`, `run_event`
  - scenarios: `runtime-cancel-rollback`
- `execution_cancel_notifies_control`
  - anchors: `runtime_loop`, `core_executor`, `run_event`
  - scenarios: `runtime-cancel-rollback`

### Scheduler Rules
- `PreemptWhenReady(control_plane, ordinary_ingress)`
  - anchors: `runtime_loop`, `core_executor`, `run_event`
  - scenarios: `control-preemption`

### Invariants
- `control_preempts_ordinary_work`
  - anchors: `runtime_loop`, `core_executor`, `run_event`
  - scenarios: `control-preemption`
- `begin_run_requires_staged_drain`
  - anchors: `runtime_loop`, `core_executor`, `run_event`
  - scenarios: `runtime-success-path`
- `begun_run_must_start_execution`
  - anchors: `runtime_loop`, `core_executor`, `run_event`
  - scenarios: `runtime-success-path`
- `execution_failure_is_handled`
  - anchors: `runtime_loop`, `core_executor`, `run_event`
  - scenarios: `runtime-failure-rollback`
- `execution_cancel_is_handled`
  - anchors: `runtime_loop`, `core_executor`, `run_event`
  - scenarios: `runtime-cancel-rollback`
- `execution_completion_is_handled`
  - anchors: `runtime_loop`, `core_executor`, `run_event`
  - scenarios: `runtime-success-path`


<!-- GENERATED_COVERAGE_END -->
