# flow_frame_loop

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `flow_run`: `FlowRunMachine` @ actor `run_engine`
- `flow_frame`: `FlowFrameMachine` @ actor `frame_engine`
- `loop_iteration`: `LoopIterationMachine` @ actor `loop_engine`

## Routes
- `flow_frame_ready_frontier_updates_run_ready_frames`: `flow_frame`.`ReadyFrontierChanged` -> `flow_run`.`RegisterReadyFrame` [Immediate]
- `flow_frame_node_release_updates_run_slots`: `flow_frame`.`NodeExecutionReleased` -> `flow_run`.`NodeExecutionReleased` [Immediate]
- `loop_request_body_frame_updates_run_pending_queue`: `loop_iteration`.`RequestBodyFrameStart` -> `flow_run`.`RegisterPendingBodyFrame` [Immediate]
- `body_frame_completed_advances_loop_iteration`: `flow_frame`.`BodyFrameCompleted` -> `loop_iteration`.`BodyFrameCompleted` [Immediate]
- `body_frame_failed_fails_loop_iteration`: `flow_frame`.`BodyFrameFailed` -> `loop_iteration`.`BodyFrameFailed` [Immediate]
- `body_frame_canceled_cancels_loop_iteration`: `flow_frame`.`BodyFrameCanceled` -> `loop_iteration`.`BodyFrameCanceled` [Immediate]
- `loop_completed_completes_parent_loop_node`: `loop_iteration`.`LoopCompleted` -> `flow_frame`.`CompleteNode` [Immediate]
- `loop_exhausted_fails_parent_loop_node`: `loop_iteration`.`LoopExhausted` -> `flow_frame`.`FailNode` [Immediate]
- `loop_failed_fails_parent_loop_node`: `loop_iteration`.`LoopFailed` -> `flow_frame`.`FailNode` [Immediate]
- `loop_canceled_cancels_parent_loop_node`: `loop_iteration`.`LoopCanceled` -> `flow_frame`.`CancelNode` [Immediate]

## Target Selectors
- `loop_completed_completes_parent_loop_node` selects `frame_id` from `Field { from_field: "parent_frame_id", allow_named_alias: true }`
- `loop_exhausted_fails_parent_loop_node` selects `frame_id` from `Field { from_field: "parent_frame_id", allow_named_alias: true }`
- `loop_failed_fails_parent_loop_node` selects `frame_id` from `Field { from_field: "parent_frame_id", allow_named_alias: true }`
- `loop_canceled_cancels_parent_loop_node` selects `frame_id` from `Field { from_field: "parent_frame_id", allow_named_alias: true }`

## Driver
- `FlowFrameLoopDriver` in `meerkat-mob/src/generated/flow_frame_loop_driver.rs`

## Transaction Plans
- `grant_node_slot_step` via `acknowledge_node_grant` / `cas_grant_node_slot` — Grant a node slot, admit a step node, and persist the updated run/frame state before spawning step work.
- `grant_node_slot_loop_start` via `acknowledge_node_grant` / `cas_start_loop` — Grant a node slot, admit a loop node, start the loop instance, and route pending-body-frame registration through run state.
- `grant_body_frame_start` via `acknowledge_body_frame_start` / `cas_grant_body_frame_start` — Acknowledge a body-frame grant, transition the loop to BodyFrameActive, create the initial body frame, and register ready work.
- `run_state_only` via `revisit_frame` / `cas_flow_state` — Apply a run-state-only routed effect such as ready-frame registration, node-slot release, or pending-body-frame registration.
- `seal_frame` via `revisit_frame` / `cas_frame_state` — Seal a frame whose tracked nodes are all terminal so the frame machine emits its typed root/body terminal effect.
- `complete_body_frame` via `advance_body_frame_after_seal` / `cas_complete_body_frame` — Persist body-frame terminalization into loop state and release the active body-frame slot before until evaluation feedback.
- `loop_request_body_frame` via `resolve_until_feedback` / `cas_loop_request_body_frame` — Persist an UntilConditionFailed replay that re-requests the next body frame through the run scheduler.
- `complete_loop` via `resolve_until_feedback` / `cas_complete_loop` — Persist a terminal loop outcome and project its routed parent-frame node transition in the same CAS bundle.

## Scheduler Rules
- `(none)`

## Structural Requirements
- `flow_loop_until_protocol_covered` — EvaluateUntilCondition is covered by the flow_loop_until_evaluation handoff protocol

## Behavioral Invariants
- `(none)`

## Coverage
### Code Anchors
- `meerkat-mob/src/runtime/flow_frame_engine.rs` — async host that executes generated frame/loop driver plans and external step/until work
- `meerkat-mob/src/generated/flow_frame_loop_driver.rs` — generated composition driver that owns frame/loop route closure and transaction bundle selection
- `meerkat-mob/src/generated/protocol_flow_loop_until_evaluation.rs` — generated until-evaluation handoff helper for loop completion feedback
- `meerkat-mob/src/runtime/loop_iteration_authority.rs` — loop iteration authority that closes until-evaluation feedback against persisted loop snapshots

### Scenarios
- `body-frame-terminal-routing` — body-frame terminal effects advance loop lifecycle and project the loop terminal outcome back into the parent frame node
- `until-evaluation-feedback` — EvaluateUntilCondition is realized by runtime-owned condition evaluation and closed by typed feedback
