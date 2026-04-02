# flow_frame_loop Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `flow_frame_loop`

### Code Anchors
- `flow_frame_engine`: `meerkat-mob/src/runtime/flow_frame_engine.rs` — async host that executes generated frame/loop driver plans and external step/until work
- `flow_frame_loop_driver`: `meerkat-mob/src/generated/flow_frame_loop_driver.rs` — generated composition driver that owns frame/loop route closure and transaction bundle selection
- `loop_until_protocol`: `meerkat-mob/src/generated/protocol_flow_loop_until_evaluation.rs` — generated until-evaluation handoff helper for loop completion feedback
- `loop_iteration_authority`: `meerkat-mob/src/runtime/loop_iteration_authority.rs` — loop iteration authority that closes until-evaluation feedback against persisted loop snapshots

### Scenarios
- `body-frame-terminal-routing` — body-frame terminal effects advance loop lifecycle and project the loop terminal outcome back into the parent frame node
- `until-evaluation-feedback` — EvaluateUntilCondition is realized by runtime-owned condition evaluation and closed by typed feedback

### Routes
- `flow_frame_ready_frontier_updates_run_ready_frames`
  - anchors: `flow_frame_engine`, `flow_frame_loop_driver`, `loop_until_protocol`, `loop_iteration_authority`
  - scenarios: `body-frame-terminal-routing`
- `flow_frame_node_release_updates_run_slots`
  - anchors: `flow_frame_engine`, `flow_frame_loop_driver`, `loop_until_protocol`, `loop_iteration_authority`
  - scenarios: `body-frame-terminal-routing`
- `loop_request_body_frame_updates_run_pending_queue`
  - anchors: `flow_frame_engine`, `flow_frame_loop_driver`, `loop_until_protocol`, `loop_iteration_authority`
  - scenarios: `body-frame-terminal-routing`
- `body_frame_completed_advances_loop_iteration`
  - anchors: `flow_frame_engine`, `flow_frame_loop_driver`, `loop_until_protocol`, `loop_iteration_authority`
  - scenarios: `body-frame-terminal-routing`
- `body_frame_failed_fails_loop_iteration`
  - anchors: `flow_frame_engine`, `flow_frame_loop_driver`, `loop_until_protocol`, `loop_iteration_authority`
  - scenarios: `body-frame-terminal-routing`
- `body_frame_canceled_cancels_loop_iteration`
  - anchors: `flow_frame_engine`, `flow_frame_loop_driver`, `loop_until_protocol`, `loop_iteration_authority`
  - scenarios: `body-frame-terminal-routing`
- `loop_completed_completes_parent_loop_node`
  - anchors: `flow_frame_engine`, `flow_frame_loop_driver`, `loop_until_protocol`, `loop_iteration_authority`
  - scenarios: `body-frame-terminal-routing`
- `loop_exhausted_fails_parent_loop_node`
  - anchors: `flow_frame_engine`, `flow_frame_loop_driver`, `loop_until_protocol`, `loop_iteration_authority`
  - scenarios: `body-frame-terminal-routing`
- `loop_failed_fails_parent_loop_node`
  - anchors: `flow_frame_engine`, `flow_frame_loop_driver`, `loop_until_protocol`, `loop_iteration_authority`
  - scenarios: `body-frame-terminal-routing`
- `loop_canceled_cancels_parent_loop_node`
  - anchors: `flow_frame_engine`, `flow_frame_loop_driver`, `loop_until_protocol`, `loop_iteration_authority`
  - scenarios: `body-frame-terminal-routing`

### Scheduler Rules
- `(none)`

### Invariants
- `flow_loop_until_protocol_covered`
  - anchors: `flow_frame_engine`, `flow_frame_loop_driver`, `loop_until_protocol`, `loop_iteration_authority`
  - scenarios: `body-frame-terminal-routing`


<!-- GENERATED_COVERAGE_END -->
