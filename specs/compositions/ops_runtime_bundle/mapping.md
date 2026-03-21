# ops_runtime_bundle Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `ops_runtime_bundle`

### Code Anchors
- `ops_vocab`: `meerkat-core/src/ops.rs` — shared async-operation vocabulary precursor
- `runtime_loop`: `meerkat-runtime/src/runtime_loop.rs` — runtime operation admission precursor
- `shell_job_manager`: `meerkat-tools/src/builtin/shell/job_manager.rs` — background async-op source precursor

### Scenarios
- `operation-event-reentry` — async-operation events re-enter runtime as operation input
- `ops-runtime-terminality` — lifecycle and runtime both observe required terminal outcomes
- `ops-control-preemption` — control-plane work still outranks ops-driven ingress

### Routes
- `op_event_enters_runtime_admission`
  - anchors: `ops_vocab`, `runtime_loop`, `shell_job_manager`
  - scenarios: `operation-event-reentry`
- `admitted_op_work_enters_ingress`
  - anchors: `ops_vocab`, `runtime_loop`, `shell_job_manager`
  - scenarios: `operation-event-reentry`
- `ingress_ready_starts_runtime_control`
  - anchors: `ops_vocab`, `runtime_loop`, `shell_job_manager`
  - scenarios: `operation-event-reentry`
- `runtime_control_starts_execution`
  - anchors: `ops_vocab`, `runtime_loop`, `shell_job_manager`
  - scenarios: `operation-event-reentry`
- `execution_boundary_updates_ingress`
  - anchors: `ops_vocab`, `runtime_loop`, `shell_job_manager`
  - scenarios: `operation-event-reentry`
- `execution_completion_updates_ingress`
  - anchors: `ops_vocab`, `runtime_loop`, `shell_job_manager`
  - scenarios: `operation-event-reentry`
- `execution_completion_notifies_control`
  - anchors: `ops_vocab`, `runtime_loop`, `shell_job_manager`
  - scenarios: `operation-event-reentry`
- `execution_failure_updates_ingress`
  - anchors: `ops_vocab`, `runtime_loop`, `shell_job_manager`
  - scenarios: `operation-event-reentry`
- `execution_failure_notifies_control`
  - anchors: `ops_vocab`, `runtime_loop`, `shell_job_manager`
  - scenarios: `operation-event-reentry`
- `execution_cancel_updates_ingress`
  - anchors: `ops_vocab`, `runtime_loop`, `shell_job_manager`
  - scenarios: `operation-event-reentry`
- `execution_cancel_notifies_control`
  - anchors: `ops_vocab`, `runtime_loop`, `shell_job_manager`
  - scenarios: `operation-event-reentry`
- `ops_barrier_satisfied_enters_turn_execution`
  - anchors: `ops_vocab`, `runtime_loop`, `shell_job_manager`
  - scenarios: `operation-event-reentry`

### Scheduler Rules
- `PreemptWhenReady(control_plane, ordinary_ingress)`
  - anchors: `ops_vocab`, `runtime_loop`, `shell_job_manager`
  - scenarios: `ops-control-preemption`

### Invariants
- `async_op_events_reenter_runtime_via_operation_input`
  - anchors: `ops_vocab`, `runtime_loop`, `shell_job_manager`
  - scenarios: `operation-event-reentry`
- `admitted_op_work_flows_into_ingress`
  - anchors: `ops_vocab`, `runtime_loop`, `shell_job_manager`
  - scenarios: `operation-event-reentry`
- `op_execution_failure_is_handled`
  - anchors: `ops_vocab`, `runtime_loop`, `shell_job_manager`
  - scenarios: `operation-event-reentry`
- `op_execution_cancel_is_handled`
  - anchors: `ops_vocab`, `runtime_loop`, `shell_job_manager`
  - scenarios: `operation-event-reentry`
- `barrier_satisfaction_handoff_protocol_covered`
  - anchors: `ops_vocab`, `runtime_loop`, `shell_job_manager`
  - scenarios: `operation-event-reentry`


<!-- GENERATED_COVERAGE_END -->
