# LoopIterationMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `2`
- Rust owner: `meerkat-mob` / `generated::loop_iteration`

## State
- Phase enum: `Absent | Running | Completed | Exhausted | Failed | Canceled`
- `loop_instance_id`: `LoopInstanceId`
- `parent_frame_id`: `FrameId`
- `parent_node_id`: `FlowNodeId`
- `loop_id`: `LoopId`
- `depth`: `u32`
- `stage`: `LoopIterationStage`
- `current_iteration`: `u32`
- `last_completed_iteration`: `u32`
- `max_iterations`: `u32`
- `active_body_frame_id`: `Option<FrameId>`

## Inputs
- `StartLoop`(loop_instance_id: LoopInstanceId, max_iterations: u32, parent_frame_id: FrameId, parent_node_id: FlowNodeId, loop_id: LoopId, depth: u32)
- `BodyFrameStarted`(loop_instance_id: LoopInstanceId, frame_id: FrameId, iteration: u32)
- `BodyFrameCompleted`(loop_instance_id: LoopInstanceId, iteration: u32)
- `BodyFrameFailed`(loop_instance_id: LoopInstanceId, iteration: u32)
- `BodyFrameCanceled`(loop_instance_id: LoopInstanceId, iteration: u32)
- `UntilConditionMet`(loop_instance_id: LoopInstanceId, iteration: u32)
- `UntilConditionFailed`(loop_instance_id: LoopInstanceId, iteration: u32)
- `CancelLoop`(loop_instance_id: LoopInstanceId)

## Effects
- `RequestBodyFrameStart`(loop_instance_id: LoopInstanceId, depth: u32)
- `EvaluateUntilCondition`(loop_instance_id: LoopInstanceId, iteration: u32, parent_frame_id: FrameId, parent_node_id: FlowNodeId, loop_id: LoopId)
- `LoopCompleted`(loop_instance_id: LoopInstanceId, parent_frame_id: FrameId, parent_node_id: FlowNodeId)
- `LoopExhausted`(loop_instance_id: LoopInstanceId, parent_frame_id: FrameId, parent_node_id: FlowNodeId)
- `LoopFailed`(loop_instance_id: LoopInstanceId, parent_frame_id: FrameId, parent_node_id: FlowNodeId)
- `LoopCanceled`(loop_instance_id: LoopInstanceId, parent_frame_id: FrameId, parent_node_id: FlowNodeId)

## Invariants

## Transitions
### `StartLoop`
- From: `Absent`
- On: `StartLoop`(loop_instance_id, max_iterations, parent_frame_id, parent_node_id, loop_id, depth)
- Emits: `RequestBodyFrameStart`
- To: `Running`

### `BodyFrameStarted`
- From: `Running`
- On: `BodyFrameStarted`(loop_instance_id, frame_id, iteration)
- Guards:
  - `loop_identity_matches`
  - `awaiting_body_frame`
  - `iteration_matches_current`
- To: `Running`

### `BodyFrameCompleted`
- From: `Running`
- On: `BodyFrameCompleted`(loop_instance_id, iteration)
- Guards:
  - `loop_identity_matches`
  - `body_frame_active`
  - `iteration_matches_current`
- Emits: `EvaluateUntilCondition`
- To: `Running`

### `UntilConditionMet`
- From: `Running`
- On: `UntilConditionMet`(loop_instance_id, iteration)
- Guards:
  - `loop_identity_matches`
  - `awaiting_until`
  - `iteration_matches_last_completed`
- Emits: `LoopCompleted`
- To: `Completed`

### `UntilConditionFailed`
- From: `Running`
- On: `UntilConditionFailed`(loop_instance_id, iteration)
- Guards:
  - `loop_identity_matches`
  - `awaiting_until`
  - `iteration_matches_last_completed`
  - `iterations_not_exhausted`
- Emits: `RequestBodyFrameStart`
- To: `Running`

### `ExhaustedIterations`
- From: `Running`
- On: `UntilConditionFailed`(loop_instance_id, iteration)
- Guards:
  - `loop_identity_matches`
  - `awaiting_until`
  - `iteration_matches_last_completed`
  - `iterations_exhausted`
- Emits: `LoopExhausted`
- To: `Exhausted`

### `BodyFrameFailed`
- From: `Running`
- On: `BodyFrameFailed`(loop_instance_id, iteration)
- Guards:
  - `loop_identity_matches`
  - `body_frame_active`
  - `iteration_matches_current`
- Emits: `LoopFailed`
- To: `Failed`

### `BodyFrameCanceled`
- From: `Running`
- On: `BodyFrameCanceled`(loop_instance_id, iteration)
- Guards:
  - `loop_identity_matches`
  - `body_frame_active`
  - `iteration_matches_current`
- Emits: `LoopCanceled`
- To: `Canceled`

### `CancelLoop`
- From: `Running`
- On: `CancelLoop`(loop_instance_id)
- Guards:
  - `loop_identity_matches`
- Emits: `LoopCanceled`
- To: `Canceled`

## Coverage
### Code Anchors
- `meerkat-machine-schema/src/catalog/loop_iteration.rs` — formal LoopIterationMachine schema (Phase 0 stub)

### Scenarios
- `start-iterate-complete` — loop starts, body frames execute, until condition terminates it (Phase 1 complete)
