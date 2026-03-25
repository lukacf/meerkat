# continuation_runtime_bundle Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `continuation_runtime_bundle`

### Code Anchors
- `keep_alive_runtime_adapter`: `meerkat-runtime/src/session_adapter.rs` — runtime-owned keep-alive drain lifecycle (maybe_spawn_comms_drain)
- `runtime_comms_drain`: `meerkat-runtime/src/comms_drain.rs` — comms inbox drain feeding typed inputs into the runtime adapter
- `agent_comms_impl`: `meerkat-core/src/agent/comms_impl.rs` — terminal peer response continuation precursor
- `agent_runner`: `meerkat-core/src/agent/runner.rs` — continuation acceptance precursor

### Scenarios
- `terminal-response-continuation` — terminal peer responses schedule continuation through runtime-owned admission
- `keep-alive-continuation` — keep-alive continuation runs through the canonical runtime path
- `continuation-control-preemption` — control-plane work preempts continuation ingress scheduling

### Routes
- `continuation_enters_ingress`
  - anchors: `keep_alive_runtime_adapter`, `runtime_comms_drain`, `agent_comms_impl`, `agent_runner`
  - scenarios: `terminal-response-continuation`, `keep-alive-continuation`
- `continuation_ingress_ready_starts_runtime_control`
  - anchors: `keep_alive_runtime_adapter`, `runtime_comms_drain`, `agent_comms_impl`, `agent_runner`
  - scenarios: `terminal-response-continuation`, `keep-alive-continuation`
- `continuation_runtime_control_starts_execution`
  - anchors: `keep_alive_runtime_adapter`, `runtime_comms_drain`, `agent_comms_impl`, `agent_runner`
  - scenarios: `terminal-response-continuation`, `keep-alive-continuation`
- `continuation_execution_completion_updates_ingress`
  - anchors: `keep_alive_runtime_adapter`, `runtime_comms_drain`, `agent_comms_impl`, `agent_runner`
  - scenarios: `terminal-response-continuation`, `keep-alive-continuation`
- `continuation_execution_completion_notifies_control`
  - anchors: `keep_alive_runtime_adapter`, `runtime_comms_drain`, `agent_comms_impl`, `agent_runner`
  - scenarios: `terminal-response-continuation`, `keep-alive-continuation`
- `continuation_execution_boundary_updates_ingress`
  - anchors: `keep_alive_runtime_adapter`, `runtime_comms_drain`, `agent_comms_impl`, `agent_runner`
  - scenarios: `terminal-response-continuation`, `keep-alive-continuation`
- `continuation_execution_failure_updates_ingress`
  - anchors: `keep_alive_runtime_adapter`, `runtime_comms_drain`, `agent_comms_impl`, `agent_runner`
  - scenarios: `terminal-response-continuation`, `keep-alive-continuation`
- `continuation_execution_failure_notifies_control`
  - anchors: `keep_alive_runtime_adapter`, `runtime_comms_drain`, `agent_comms_impl`, `agent_runner`
  - scenarios: `terminal-response-continuation`, `keep-alive-continuation`
- `continuation_execution_cancel_updates_ingress`
  - anchors: `keep_alive_runtime_adapter`, `runtime_comms_drain`, `agent_comms_impl`, `agent_runner`
  - scenarios: `terminal-response-continuation`, `keep-alive-continuation`
- `continuation_execution_cancel_notifies_control`
  - anchors: `keep_alive_runtime_adapter`, `runtime_comms_drain`, `agent_comms_impl`, `agent_runner`
  - scenarios: `terminal-response-continuation`, `keep-alive-continuation`

### Scheduler Rules
- `PreemptWhenReady(control_plane, ordinary_ingress)`
  - anchors: `keep_alive_runtime_adapter`, `runtime_comms_drain`, `agent_comms_impl`, `agent_runner`
  - scenarios: `continuation-control-preemption`

### Invariants
- `continuation_uses_canonical_runtime_admission`
  - anchors: `keep_alive_runtime_adapter`, `runtime_comms_drain`, `agent_comms_impl`, `agent_runner`
  - scenarios: `terminal-response-continuation`, `keep-alive-continuation`
- `continuation_begin_run_requires_staged_drain`
  - anchors: `keep_alive_runtime_adapter`, `runtime_comms_drain`, `agent_comms_impl`, `agent_runner`
  - scenarios: `terminal-response-continuation`, `keep-alive-continuation`
- `continuation_execution_completion_is_handled`
  - anchors: `keep_alive_runtime_adapter`, `runtime_comms_drain`, `agent_comms_impl`, `agent_runner`
  - scenarios: `terminal-response-continuation`, `keep-alive-continuation`
- `continuation_execution_failure_is_handled`
  - anchors: `keep_alive_runtime_adapter`, `runtime_comms_drain`, `agent_comms_impl`, `agent_runner`
  - scenarios: `terminal-response-continuation`, `keep-alive-continuation`
- `continuation_execution_cancel_is_handled`
  - anchors: `keep_alive_runtime_adapter`, `runtime_comms_drain`, `agent_comms_impl`, `agent_runner`
  - scenarios: `terminal-response-continuation`, `keep-alive-continuation`
- `control_preempts_continuation_ingress`
  - anchors: `keep_alive_runtime_adapter`, `runtime_comms_drain`, `agent_comms_impl`, `agent_runner`
  - scenarios: `continuation-control-preemption`


<!-- GENERATED_COVERAGE_END -->
