# FlowFrameMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `FlowFrameMachine`

### Code Anchors
- `flow_frame_schema`: `meerkat-machine-schema/src/catalog/flow_frame.rs` — formal FlowFrameMachine schema (Phase 0 stub)

### Scenarios
- `start-run-terminalize` — frame starts, admits nodes, and terminalizes (Phase 1 complete)

### Transitions
- `StartFrame`
  - anchors: `flow_frame_schema`
  - scenarios: `start-run-terminalize`
- `AdmitNextReadyNode_StepRun`
  - anchors: `flow_frame_schema`
  - scenarios: `start-run-terminalize`
- `AdmitNextReadyNode_LoopRun`
  - anchors: `flow_frame_schema`
  - scenarios: `start-run-terminalize`
- `AdmitNextReadyNode_Skip`
  - anchors: `flow_frame_schema`
  - scenarios: `start-run-terminalize`
- `AdmitNextReadyNode_Fail`
  - anchors: `flow_frame_schema`
  - scenarios: `start-run-terminalize`
- `CompleteNode`
  - anchors: `flow_frame_schema`
  - scenarios: `start-run-terminalize`
- `RecordNodeOutput`
  - anchors: `flow_frame_schema`
  - scenarios: `start-run-terminalize`
- `FailNode`
  - anchors: `flow_frame_schema`
  - scenarios: `start-run-terminalize`
- `SkipNode`
  - anchors: `flow_frame_schema`
  - scenarios: `start-run-terminalize`
- `CancelNode`
  - anchors: `flow_frame_schema`
  - scenarios: `start-run-terminalize`
- `TerminalizeCompleted`
  - anchors: `flow_frame_schema`
  - scenarios: `start-run-terminalize`
- `TerminalizeFailed`
  - anchors: `flow_frame_schema`
  - scenarios: `start-run-terminalize`
- `TerminalizeCanceled`
  - anchors: `flow_frame_schema`
  - scenarios: `start-run-terminalize`

### Effects
- `ReadyFrontierChanged`
  - anchors: `flow_frame_schema`
  - scenarios: `start-run-terminalize`
- `AdmitStepWork`
  - anchors: `flow_frame_schema`
  - scenarios: `start-run-terminalize`
- `StartLoopNode`
  - anchors: `flow_frame_schema`
  - scenarios: `start-run-terminalize`
- `PersistStepOutput`
  - anchors: `flow_frame_schema`
  - scenarios: `start-run-terminalize`
- `NodeExecutionReleased`
  - anchors: `flow_frame_schema`
  - scenarios: `start-run-terminalize`
- `FrameTerminalized`
  - anchors: `flow_frame_schema`
  - scenarios: `start-run-terminalize`

### Invariants
- `ready_queue_membership_matches_ready_status`
  - anchors: `flow_frame_schema`
  - scenarios: `start-run-terminalize`


<!-- GENERATED_COVERAGE_END -->
