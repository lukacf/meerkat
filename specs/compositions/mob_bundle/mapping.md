# mob_bundle Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `mob_bundle`

### Code Anchors
- `mob_runtime_actor`: `meerkat-mob/src/runtime/actor.rs` — mob orchestration precursor
- `mob_member_handle`: `meerkat-mob/src/runtime/handle.rs` — member-directed delivery capability
- `flow_runtime`: `meerkat-mob/src/runtime/flow.rs` — flow dispatch precursor
- `peer_runtime`: `meerkat-comms/src/runtime/comms_runtime.rs` — mob member peer communication precursor
- `wasm_example_031`: `examples/031-wasm-mini-diplomacy-sh/web/src/main.ts` — mob-based WASM example coverage
- `wasm_example_032`: `examples/032-wasm-webcm-agent/web/src/main.ts` — browser mob workflow coverage
- `wasm_example_033`: `examples/033-the-office-demo-sh/web/src/main.ts` — browser local-tool + mob coverage

### Scenarios
- `mob-flow-dispatch` — flow step work enters runtime only through canonical admission
- `mob-child-report-back` — mob-backed child work reports progress and terminality through ops lifecycle
- `mob-peer-orchestration` — member-directed communication and orchestration stay inside the mob stack
- `wasm-mob-examples` — browser mob examples continue to fit the canonical mob/comms/runtime model

### Routes
- `mob_supervisor_activation_starts_lifecycle`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-peer-orchestration`, `wasm-mob-examples`
- `flow_step_dispatch_enters_runtime_admission`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-flow-dispatch`, `wasm-mob-examples`
- `mob_flow_activation_starts_flow_run`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-flow-dispatch`, `wasm-mob-examples`
- `mob_flow_activation_marks_lifecycle_run`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-flow-dispatch`, `wasm-mob-examples`
- `mob_async_op_event_enters_runtime_admission`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-child-report-back`
- `mob_peer_candidate_enters_runtime_admission`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-peer-orchestration`, `wasm-mob-examples`
- `mob_admitted_work_enters_ingress`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-peer-orchestration`, `wasm-mob-examples`
- `mob_ingress_ready_starts_runtime_control`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-peer-orchestration`, `wasm-mob-examples`
- `mob_runtime_control_starts_execution`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-peer-orchestration`, `wasm-mob-examples`
- `mob_flow_terminalization_completes_orchestrator`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-flow-dispatch`, `wasm-mob-examples`
- `mob_flow_deactivation_finishes_lifecycle_run`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-flow-dispatch`, `wasm-mob-examples`
- `mob_execution_boundary_updates_ingress`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-peer-orchestration`, `wasm-mob-examples`
- `mob_execution_completion_updates_ingress`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-peer-orchestration`, `wasm-mob-examples`
- `mob_execution_completion_notifies_control`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-peer-orchestration`, `wasm-mob-examples`
- `mob_execution_failure_updates_ingress`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-peer-orchestration`, `wasm-mob-examples`
- `mob_execution_failure_notifies_control`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-peer-orchestration`, `wasm-mob-examples`
- `mob_execution_cancel_updates_ingress`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-peer-orchestration`, `wasm-mob-examples`
- `mob_execution_cancel_notifies_control`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-peer-orchestration`, `wasm-mob-examples`

### Scheduler Rules
- `PreemptWhenReady(control_plane, ordinary_ingress)`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-peer-orchestration`

### Invariants
- `mob_supervisor_activation_starts_lifecycle`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-peer-orchestration`, `wasm-mob-examples`
- `mob_flow_activation_starts_flow_run`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-flow-dispatch`, `wasm-mob-examples`
- `mob_flow_activation_marks_lifecycle_run`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-flow-dispatch`, `wasm-mob-examples`
- `flow_dispatch_uses_canonical_runtime_admission`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-flow-dispatch`, `wasm-mob-examples`
- `mob_async_lifecycle_events_use_operation_input`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-child-report-back`
- `mob_peer_work_uses_canonical_runtime_admission`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-peer-orchestration`, `wasm-mob-examples`
- `mob_runtime_work_flows_into_ingress`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-flow-dispatch`, `wasm-mob-examples`
- `mob_flow_terminalization_completes_orchestrator`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-flow-dispatch`, `wasm-mob-examples`
- `mob_flow_deactivation_finishes_lifecycle_run`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-flow-dispatch`, `wasm-mob-examples`
- `mob_execution_failure_is_handled`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-peer-orchestration`, `wasm-mob-examples`
- `mob_execution_cancel_is_handled`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-peer-orchestration`, `wasm-mob-examples`
- `control_preempts_mob_ingress`
  - anchors: `mob_runtime_actor`, `mob_member_handle`, `flow_runtime`, `peer_runtime`, `wasm_example_031`, `wasm_example_032`, `wasm_example_033`
  - scenarios: `mob-peer-orchestration`


<!-- GENERATED_COVERAGE_END -->
