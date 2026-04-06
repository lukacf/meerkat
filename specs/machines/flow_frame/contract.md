# FlowFrameMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `3`
- Rust owner: `meerkat-mob` / `generated::flow_frame`

## State
- Phase enum: `Absent | Running | Completed | Failed | Canceled`
- `frame_id`: `FrameId`
- `frame_scope`: `FrameScope`
- `loop_instance_id`: `LoopInstanceId`
- `iteration`: `u32`
- `last_admitted_node`: `FlowNodeId`
- `tracked_nodes`: `Set<FlowNodeId>`
- `ordered_nodes`: `Seq<FlowNodeId>`
- `node_kind`: `Map<FlowNodeId, FlowNodeKind>`
- `node_dependencies`: `Map<FlowNodeId, Seq<FlowNodeId>>`
- `node_dependency_modes`: `Map<FlowNodeId, DependencyMode>`
- `node_branches`: `Map<FlowNodeId, Option<BranchId>>`
- `branch_winners`: `Set<BranchId>`
- `node_status`: `Map<FlowNodeId, NodeRunStatus>`
- `ready_queue`: `Seq<FlowNodeId>`
- `output_recorded`: `Map<FlowNodeId, Bool>`
- `node_condition_results`: `Map<FlowNodeId, Option<Bool>>`

## Inputs
- `StartRootFrame`(frame_id: FrameId, tracked_nodes: Set<FlowNodeId>, ordered_nodes: Seq<FlowNodeId>, node_kind: Map<FlowNodeId, FlowNodeKind>, node_dependencies: Map<FlowNodeId, Seq<FlowNodeId>>, node_dependency_modes: Map<FlowNodeId, DependencyMode>, node_branches: Map<FlowNodeId, Option<BranchId>>)
- `StartBodyFrame`(frame_id: FrameId, loop_instance_id: LoopInstanceId, iteration: u32, tracked_nodes: Set<FlowNodeId>, ordered_nodes: Seq<FlowNodeId>, node_kind: Map<FlowNodeId, FlowNodeKind>, node_dependencies: Map<FlowNodeId, Seq<FlowNodeId>>, node_dependency_modes: Map<FlowNodeId, DependencyMode>, node_branches: Map<FlowNodeId, Option<BranchId>>)
- `AdmitNextReadyNode`
- `CompleteNode`(node_id: FlowNodeId)
- `RecordNodeOutput`(node_id: FlowNodeId)
- `FailNode`(node_id: FlowNodeId)
- `SkipNode`(node_id: FlowNodeId)
- `CancelNode`(node_id: FlowNodeId)
- `SealFrame`

## Effects
- `ReadyFrontierChanged`(frame_id: FrameId)
- `AdmitStepWork`(frame_id: FrameId, node_id: FlowNodeId)
- `StartLoopNode`(frame_id: FrameId, node_id: FlowNodeId)
- `PersistStepOutput`(frame_id: FrameId, node_id: FlowNodeId)
- `NodeExecutionReleased`(frame_id: FrameId, node_id: FlowNodeId)
- `RootFrameCompleted`(frame_id: FrameId)
- `RootFrameFailed`(frame_id: FrameId)
- `RootFrameCanceled`(frame_id: FrameId)
- `BodyFrameCompleted`(frame_id: FrameId, loop_instance_id: LoopInstanceId, iteration: u32)
- `BodyFrameFailed`(frame_id: FrameId, loop_instance_id: LoopInstanceId, iteration: u32)
- `BodyFrameCanceled`(frame_id: FrameId, loop_instance_id: LoopInstanceId, iteration: u32)

## Helpers
- `NodeAdmissionEligible`(node_id: FlowNodeId) -> `Bool`
- `AllDepsCompleted`(node_id: FlowNodeId) -> `Bool`
- `AnyDepCompleted`(node_id: FlowNodeId) -> `Bool`
- `AllNodesTerminal`() -> `Bool`

## Invariants
- `ready_queue_membership_matches_ready_status`

## Transitions
### `StartRootFrame`
- From: `Absent`
- On: `StartRootFrame`(frame_id, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches)
- Emits: `ReadyFrontierChanged`
- To: `Running`

### `StartBodyFrame`
- From: `Absent`
- On: `StartBodyFrame`(frame_id, loop_instance_id, iteration, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches)
- Emits: `ReadyFrontierChanged`
- To: `Running`

### `AdmitNextReadyNode_StepRun`
- From: `Running`
- On: `AdmitNextReadyNode`()
- Guards:
  - `ready_queue_non_empty`
  - `head_is_step`
  - `head_deps_eligible_for_run`
- Emits: `AdmitStepWork`, `ReadyFrontierChanged`
- To: `Running`

### `AdmitNextReadyNode_LoopRun`
- From: `Running`
- On: `AdmitNextReadyNode`()
- Guards:
  - `ready_queue_non_empty`
  - `head_is_loop`
  - `head_deps_eligible_for_run`
- Emits: `StartLoopNode`, `NodeExecutionReleased`, `ReadyFrontierChanged`
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

### `CompleteNode_Step`
- From: `Running`
- On: `CompleteNode`(node_id)
- Guards:
  - `node_is_running`
  - `node_is_step`
- Emits: `NodeExecutionReleased`, `ReadyFrontierChanged`
- To: `Running`

### `CompleteNode_Loop`
- From: `Running`
- On: `CompleteNode`(node_id)
- Guards:
  - `node_is_running`
  - `node_is_loop`
- Emits: `ReadyFrontierChanged`
- To: `Running`

### `RecordNodeOutput`
- From: `Running`
- On: `RecordNodeOutput`(node_id)
- Emits: `PersistStepOutput`
- To: `Running`

### `FailNode_Step`
- From: `Running`
- On: `FailNode`(node_id)
- Guards:
  - `node_is_running`
  - `node_is_step`
- Emits: `NodeExecutionReleased`, `ReadyFrontierChanged`
- To: `Running`

### `FailNode_Loop`
- From: `Running`
- On: `FailNode`(node_id)
- Guards:
  - `node_is_running`
  - `node_is_loop`
- Emits: `ReadyFrontierChanged`
- To: `Running`

### `SkipNode_Step`
- From: `Running`
- On: `SkipNode`(node_id)
- Guards:
  - `node_is_running`
  - `node_is_step`
- Emits: `NodeExecutionReleased`, `ReadyFrontierChanged`
- To: `Running`

### `SkipNode_Loop`
- From: `Running`
- On: `SkipNode`(node_id)
- Guards:
  - `node_is_running`
  - `node_is_loop`
- Emits: `ReadyFrontierChanged`
- To: `Running`

### `CancelNode_Step`
- From: `Running`
- On: `CancelNode`(node_id)
- Guards:
  - `node_is_running`
  - `node_is_step`
- Emits: `NodeExecutionReleased`, `ReadyFrontierChanged`
- To: `Running`

### `CancelNode_Loop`
- From: `Running`
- On: `CancelNode`(node_id)
- Guards:
  - `node_is_running`
  - `node_is_loop`
- Emits: `ReadyFrontierChanged`
- To: `Running`

### `SealRootFrameCanceled`
- From: `Running`
- On: `SealFrame`()
- Guards:
  - `all_nodes_terminal`
  - `root_frame`
  - `has_canceled_nodes`
- Emits: `RootFrameCanceled`
- To: `Canceled`

### `SealRootFrameFailed`
- From: `Running`
- On: `SealFrame`()
- Guards:
  - `all_nodes_terminal`
  - `root_frame`
  - `no_canceled_nodes`
  - `has_failed_nodes`
- Emits: `RootFrameFailed`
- To: `Failed`

### `SealRootFrameCompleted`
- From: `Running`
- On: `SealFrame`()
- Guards:
  - `all_nodes_terminal`
  - `root_frame`
  - `no_canceled_nodes`
  - `no_failed_nodes`
- Emits: `RootFrameCompleted`
- To: `Completed`

### `SealBodyFrameCanceled`
- From: `Running`
- On: `SealFrame`()
- Guards:
  - `all_nodes_terminal`
  - `body_frame`
  - `has_canceled_nodes`
- Emits: `BodyFrameCanceled`
- To: `Canceled`

### `SealBodyFrameFailed`
- From: `Running`
- On: `SealFrame`()
- Guards:
  - `all_nodes_terminal`
  - `body_frame`
  - `no_canceled_nodes`
  - `has_failed_nodes`
- Emits: `BodyFrameFailed`
- To: `Failed`

### `SealBodyFrameCompleted`
- From: `Running`
- On: `SealFrame`()
- Guards:
  - `all_nodes_terminal`
  - `body_frame`
  - `no_canceled_nodes`
  - `no_failed_nodes`
- Emits: `BodyFrameCompleted`
- To: `Completed`

## Coverage
### Code Anchors
- `meerkat-machine-schema/src/catalog/flow_frame.rs` — formal FlowFrameMachine schema (Phase 0 stub)

### Scenarios
- `start-run-terminalize` — frame starts, admits nodes, and terminalizes (Phase 1 complete)
