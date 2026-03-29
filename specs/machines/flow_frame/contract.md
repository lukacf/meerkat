# FlowFrameMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-mob` / `generated::flow_frame`

## State
- Phase enum: `Absent | Running | Completed | Failed | Canceled`
- `frame_id`: `FrameId`
- `last_admitted_node`: `FlowNodeId`
- `tracked_nodes`: `Set<FlowNodeId>`
- `ordered_nodes`: `Seq<FlowNodeId>`
- `node_kind`: `Map<FlowNodeId, FlowNodeKind>`
- `node_dependencies`: `Map<FlowNodeId, Seq<FlowNodeId>>`
- `node_dependency_modes`: `Map<FlowNodeId, DependencyMode>`
- `node_branches`: `Map<FlowNodeId, Option<BranchId>>`
- `node_status`: `Map<FlowNodeId, NodeRunStatus>`
- `ready_queue`: `Seq<FlowNodeId>`
- `output_recorded`: `Map<FlowNodeId, Bool>`
- `node_condition_results`: `Map<FlowNodeId, Option<Bool>>`

## Inputs
- `StartFrame`(frame_id: FrameId, tracked_nodes: Set<FlowNodeId>, ordered_nodes: Seq<FlowNodeId>, node_kind: Map<FlowNodeId, FlowNodeKind>, node_dependencies: Map<FlowNodeId, Seq<FlowNodeId>>, node_dependency_modes: Map<FlowNodeId, DependencyMode>, node_branches: Map<FlowNodeId, Option<BranchId>>)
- `AdmitNextReadyNode`
- `CompleteNode`(node_id: FlowNodeId)
- `RecordNodeOutput`(node_id: FlowNodeId)
- `FailNode`(node_id: FlowNodeId)
- `SkipNode`(node_id: FlowNodeId)
- `CancelNode`(node_id: FlowNodeId)
- `TerminalizeCompleted`
- `TerminalizeFailed`
- `TerminalizeCanceled`

## Effects
- `ReadyFrontierChanged`(frame_id: FrameId)
- `AdmitStepWork`(frame_id: FrameId, node_id: FlowNodeId)
- `StartLoopNode`(frame_id: FrameId, node_id: FlowNodeId)
- `PersistStepOutput`(frame_id: FrameId, node_id: FlowNodeId)
- `NodeExecutionReleased`(frame_id: FrameId, node_id: FlowNodeId)
- `FrameTerminalized`(frame_id: FrameId, status: FlowFrameStatus)

## Helpers
- `NodeAdmissionEligible`(node_id: FlowNodeId) -> `Bool`
- `AllDepsCompleted`(node_id: FlowNodeId) -> `Bool`
- `AnyDepCompleted`(node_id: FlowNodeId) -> `Bool`
- `AllNodesTerminal`() -> `Bool`

## Invariants
- `ready_queue_membership_matches_ready_status`

## Transitions
### `StartFrame`
- From: `Absent`
- On: `StartFrame`(frame_id, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches)
- Emits: `ReadyFrontierChanged`
- To: `Running`

### `AdmitNextReadyNode_StepRun`
- From: `Running`
- On: `AdmitNextReadyNode`()
- Guards:
  - `ready_queue_non_empty`
  - `head_is_step`
  - `head_deps_eligible_for_run`
- Emits: `AdmitStepWork`
- To: `Running`

### `AdmitNextReadyNode_LoopRun`
- From: `Running`
- On: `AdmitNextReadyNode`()
- Guards:
  - `ready_queue_non_empty`
  - `head_is_loop`
  - `head_deps_eligible_for_run`
- Emits: `StartLoopNode`
- To: `Running`

### `AdmitNextReadyNode_Skip`
- From: `Running`
- On: `AdmitNextReadyNode`()
- Guards:
  - `ready_queue_non_empty`
  - `head_should_skip`
- Emits: `NodeExecutionReleased`, `ReadyFrontierChanged`
- To: `Running`

### `AdmitNextReadyNode_Fail`
- From: `Running`
- On: `AdmitNextReadyNode`()
- Guards:
  - `ready_queue_non_empty`
  - `head_should_fail`
- Emits: `NodeExecutionReleased`, `ReadyFrontierChanged`
- To: `Running`

### `CompleteNode`
- From: `Running`
- On: `CompleteNode`(node_id)
- Guards:
  - `node_is_running`
- Emits: `NodeExecutionReleased`, `ReadyFrontierChanged`
- To: `Running`

### `RecordNodeOutput`
- From: `Running`
- On: `RecordNodeOutput`(node_id)
- Emits: `PersistStepOutput`
- To: `Running`

### `FailNode`
- From: `Running`
- On: `FailNode`(node_id)
- Guards:
  - `node_is_running`
- Emits: `NodeExecutionReleased`, `ReadyFrontierChanged`
- To: `Running`

### `SkipNode`
- From: `Running`
- On: `SkipNode`(node_id)
- Guards:
  - `node_is_running`
- Emits: `NodeExecutionReleased`, `ReadyFrontierChanged`
- To: `Running`

### `CancelNode`
- From: `Running`
- On: `CancelNode`(node_id)
- Guards:
  - `node_is_running`
- Emits: `NodeExecutionReleased`, `ReadyFrontierChanged`
- To: `Running`

### `TerminalizeCompleted`
- From: `Running`
- On: `TerminalizeCompleted`()
- Guards:
  - `all_nodes_terminal`
- Emits: `FrameTerminalized`
- To: `Completed`

### `TerminalizeFailed`
- From: `Running`, `Absent`
- On: `TerminalizeFailed`()
- Emits: `FrameTerminalized`
- To: `Failed`

### `TerminalizeCanceled`
- From: `Running`, `Absent`
- On: `TerminalizeCanceled`()
- Emits: `FrameTerminalized`
- To: `Canceled`

## Coverage
### Code Anchors
- `meerkat-machine-schema/src/catalog/flow_frame.rs` — formal FlowFrameMachine schema (Phase 0 stub)

### Scenarios
- `start-run-terminalize` — frame starts, admits nodes, and terminalizes (Phase 1 complete)
