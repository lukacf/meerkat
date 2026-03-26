# surface_event_runtime_bundle Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `surface_event_runtime_bundle`

### Code Anchors
- `cli_stdin_events`: `meerkat-cli/src/stdin_events.rs` — CLI external-event ingestion precursor
- `rest_event_surface`: `meerkat-rest/src/lib.rs` — REST external-event surface precursor
- `rpc_event_surface`: `meerkat-rpc/src/handlers/event.rs` — JSON-RPC external-event surface precursor
- `wasm_runtime_surface`: `meerkat-web-runtime/src/lib.rs` — WASM/browser external-event surface precursor
- `surface_event_runtime_loop`: `meerkat-runtime/src/runtime_loop.rs` — canonical external-event surface routing (input_to_append, input_to_prompt)

### Scenarios
- `cli-surface-event-admission` — CLI stdin and host-driven external events use canonical runtime admission
- `rest-surface-event-admission` — REST external events use canonical runtime admission
- `rpc-surface-event-admission` — JSON-RPC external events use canonical runtime admission
- `wasm-surface-event-admission` — browser/WASM external events use canonical runtime admission
- `surface-event-failure` — surface-originated runs still route failure through ingress and control
- `surface-control-preemption` — control-plane work still preempts surface-originated ingress

### Routes
- `surface_event_enters_ingress`
  - anchors: `cli_stdin_events`, `rest_event_surface`, `rpc_event_surface`, `wasm_runtime_surface`, `surface_event_runtime_loop`
  - scenarios: `cli-surface-event-admission`, `rest-surface-event-admission`, `rpc-surface-event-admission`, `wasm-surface-event-admission`
- `surface_ingress_ready_starts_runtime_control`
  - anchors: `cli_stdin_events`, `rest_event_surface`, `rpc_event_surface`, `wasm_runtime_surface`, `surface_event_runtime_loop`
  - scenarios: `cli-surface-event-admission`, `rest-surface-event-admission`, `rpc-surface-event-admission`, `wasm-surface-event-admission`
- `surface_runtime_control_starts_execution`
  - anchors: `cli_stdin_events`, `rest_event_surface`, `rpc_event_surface`, `wasm_runtime_surface`, `surface_event_runtime_loop`
  - scenarios: `cli-surface-event-admission`, `rest-surface-event-admission`, `rpc-surface-event-admission`, `wasm-surface-event-admission`
- `surface_execution_boundary_updates_ingress`
  - anchors: `cli_stdin_events`, `rest_event_surface`, `rpc_event_surface`, `wasm_runtime_surface`, `surface_event_runtime_loop`
  - scenarios: `cli-surface-event-admission`, `rest-surface-event-admission`, `rpc-surface-event-admission`, `wasm-surface-event-admission`
- `surface_execution_completion_updates_ingress`
  - anchors: `cli_stdin_events`, `rest_event_surface`, `rpc_event_surface`, `wasm_runtime_surface`, `surface_event_runtime_loop`
  - scenarios: `cli-surface-event-admission`, `rest-surface-event-admission`, `rpc-surface-event-admission`, `wasm-surface-event-admission`
- `surface_execution_completion_notifies_control`
  - anchors: `cli_stdin_events`, `rest_event_surface`, `rpc_event_surface`, `wasm_runtime_surface`, `surface_event_runtime_loop`
  - scenarios: `cli-surface-event-admission`, `rest-surface-event-admission`, `rpc-surface-event-admission`, `wasm-surface-event-admission`
- `surface_execution_failure_updates_ingress`
  - anchors: `cli_stdin_events`, `rest_event_surface`, `rpc_event_surface`, `wasm_runtime_surface`, `surface_event_runtime_loop`
  - scenarios: `surface-event-failure`
- `surface_execution_failure_notifies_control`
  - anchors: `cli_stdin_events`, `rest_event_surface`, `rpc_event_surface`, `wasm_runtime_surface`, `surface_event_runtime_loop`
  - scenarios: `surface-event-failure`
- `surface_execution_cancel_updates_ingress`
  - anchors: `cli_stdin_events`, `rest_event_surface`, `rpc_event_surface`, `wasm_runtime_surface`, `surface_event_runtime_loop`
  - scenarios: `cli-surface-event-admission`, `rest-surface-event-admission`, `rpc-surface-event-admission`, `wasm-surface-event-admission`
- `surface_execution_cancel_notifies_control`
  - anchors: `cli_stdin_events`, `rest_event_surface`, `rpc_event_surface`, `wasm_runtime_surface`, `surface_event_runtime_loop`
  - scenarios: `cli-surface-event-admission`, `rest-surface-event-admission`, `rpc-surface-event-admission`, `wasm-surface-event-admission`

### Scheduler Rules
- `PreemptWhenReady(control_plane, ordinary_ingress)`
  - anchors: `cli_stdin_events`, `rest_event_surface`, `rpc_event_surface`, `wasm_runtime_surface`, `surface_event_runtime_loop`
  - scenarios: `surface-control-preemption`

### Invariants
- `surface_event_uses_canonical_runtime_admission`
  - anchors: `cli_stdin_events`, `rest_event_surface`, `rpc_event_surface`, `wasm_runtime_surface`, `surface_event_runtime_loop`
  - scenarios: `cli-surface-event-admission`, `rest-surface-event-admission`, `rpc-surface-event-admission`, `wasm-surface-event-admission`
- `surface_event_begin_run_requires_staged_drain`
  - anchors: `cli_stdin_events`, `rest_event_surface`, `rpc_event_surface`, `wasm_runtime_surface`, `surface_event_runtime_loop`
  - scenarios: `cli-surface-event-admission`, `rest-surface-event-admission`, `rpc-surface-event-admission`, `wasm-surface-event-admission`
- `surface_event_execution_completion_is_handled`
  - anchors: `cli_stdin_events`, `rest_event_surface`, `rpc_event_surface`, `wasm_runtime_surface`, `surface_event_runtime_loop`
  - scenarios: `cli-surface-event-admission`, `rest-surface-event-admission`, `rpc-surface-event-admission`, `wasm-surface-event-admission`
- `surface_event_execution_failure_is_handled`
  - anchors: `cli_stdin_events`, `rest_event_surface`, `rpc_event_surface`, `wasm_runtime_surface`, `surface_event_runtime_loop`
  - scenarios: `surface-event-failure`
- `surface_event_execution_cancel_is_handled`
  - anchors: `cli_stdin_events`, `rest_event_surface`, `rpc_event_surface`, `wasm_runtime_surface`, `surface_event_runtime_loop`
  - scenarios: `cli-surface-event-admission`, `rest-surface-event-admission`, `rpc-surface-event-admission`, `wasm-surface-event-admission`
- `control_preempts_surface_event_ingress`
  - anchors: `cli_stdin_events`, `rest_event_surface`, `rpc_event_surface`, `wasm_runtime_surface`, `surface_event_runtime_loop`
  - scenarios: `surface-control-preemption`


<!-- GENERATED_COVERAGE_END -->
