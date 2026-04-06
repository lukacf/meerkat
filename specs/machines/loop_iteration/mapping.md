# LoopIterationMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `LoopIterationMachine`

### Code Anchors
- `loop_iteration_schema`: `meerkat-machine-schema/src/catalog/loop_iteration.rs` — formal LoopIterationMachine schema (Phase 0 stub)

### Scenarios
- `start-iterate-complete` — loop starts, body frames execute, until condition terminates it (Phase 1 complete)

### Transitions
- `StartLoop`
  - anchors: `loop_iteration_schema`
  - scenarios: `start-iterate-complete`
- `BodyFrameStarted`
  - anchors: `loop_iteration_schema`
  - scenarios: `start-iterate-complete`
- `BodyFrameCompleted`
  - anchors: `loop_iteration_schema`
  - scenarios: `start-iterate-complete`
- `UntilConditionMet`
  - anchors: `loop_iteration_schema`
  - scenarios: `start-iterate-complete`
- `UntilConditionFailed`
  - anchors: `loop_iteration_schema`
  - scenarios: `start-iterate-complete`
- `ExhaustedIterations`
  - anchors: `loop_iteration_schema`
  - scenarios: `start-iterate-complete`
- `BodyFrameFailed`
  - anchors: `loop_iteration_schema`
  - scenarios: `start-iterate-complete`
- `BodyFrameCanceled`
  - anchors: `loop_iteration_schema`
  - scenarios: `start-iterate-complete`
- `CancelLoop`
  - anchors: `loop_iteration_schema`
  - scenarios: `start-iterate-complete`

### Effects
- `RequestBodyFrameStart`
  - anchors: `loop_iteration_schema`
  - scenarios: `start-iterate-complete`
- `EvaluateUntilCondition`
  - anchors: `loop_iteration_schema`
  - scenarios: `start-iterate-complete`
- `LoopCompleted`
  - anchors: `loop_iteration_schema`
  - scenarios: `start-iterate-complete`
- `LoopExhausted`
  - anchors: `loop_iteration_schema`
  - scenarios: `start-iterate-complete`
- `LoopFailed`
  - anchors: `loop_iteration_schema`
  - scenarios: `start-iterate-complete`
- `LoopCanceled`
  - anchors: `loop_iteration_schema`
  - scenarios: `start-iterate-complete`

### Invariants
- `(none)`


<!-- GENERATED_COVERAGE_END -->
