# LoopIterationMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-mob` / `generated::loop_iteration`

## State
- Phase enum: `Absent | Running | Completed | Exhausted | Failed | Canceled`
- `loop_instance_id`: `LoopInstanceId`
- `current_iteration`: `u32`
- `max_iterations`: `u32`
- `active_body_frame_id`: `Option<FrameId>`
- `loop_failure_kind`: `Option<LoopFailureKind>`

## Inputs
- `StartLoop`(loop_instance_id: LoopInstanceId, max_iterations: u32)
- `BodyFrameStarted`(frame_id: FrameId)
- `BodyFrameCompleted`
- `BodyFrameFailed`
- `BodyFrameCanceled`
- `UntilConditionMet`
- `UntilConditionFailed`
- `CancelLoop`

## Effects
- `RequestBodyFrameStart`(loop_instance_id: LoopInstanceId)
- `EvaluateUntilCondition`(loop_instance_id: LoopInstanceId, iteration: u32)
- `LoopCompleted`(loop_instance_id: LoopInstanceId)
- `LoopExhausted`(loop_instance_id: LoopInstanceId)
- `LoopFailed`(loop_instance_id: LoopInstanceId)
- `LoopCanceled`(loop_instance_id: LoopInstanceId)

## Invariants

## Transitions
### `StartLoop`
- From: `Absent`
- On: `StartLoop`(loop_instance_id, max_iterations)
- Emits: `RequestBodyFrameStart`
- To: `Running`

### `BodyFrameStarted`
- From: `Running`
- On: `BodyFrameStarted`(frame_id)
- To: `Running`

### `BodyFrameCompleted`
- From: `Running`
- On: `BodyFrameCompleted`()
- Emits: `EvaluateUntilCondition`
- To: `Running`

### `UntilConditionMet`
- From: `Running`
- On: `UntilConditionMet`()
- Emits: `LoopCompleted`
- To: `Completed`

### `UntilConditionFailed`
- From: `Running`
- On: `UntilConditionFailed`()
- Guards:
  - `iterations_not_exhausted`
- Emits: `RequestBodyFrameStart`
- To: `Running`

### `ExhaustedIterations`
- From: `Running`
- On: `UntilConditionFailed`()
- Guards:
  - `iterations_exhausted`
- Emits: `LoopExhausted`
- To: `Exhausted`

### `BodyFrameFailed`
- From: `Running`
- On: `BodyFrameFailed`()
- Emits: `LoopFailed`
- To: `Failed`

### `BodyFrameCanceled`
- From: `Running`
- On: `BodyFrameCanceled`()
- Emits: `LoopCanceled`
- To: `Canceled`

### `CancelLoop`
- From: `Running`
- On: `CancelLoop`()
- Emits: `LoopCanceled`
- To: `Canceled`

## Coverage
### Code Anchors
- `meerkat-machine-schema/src/catalog/loop_iteration.rs` — formal LoopIterationMachine schema (Phase 0 stub)

### Scenarios
- `start-iterate-complete` — loop starts, body frames execute, until condition terminates it (Phase 1 complete)
