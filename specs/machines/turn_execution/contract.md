# TurnExecutionMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-core` / `generated::turn_execution`

## State
- Phase enum: `Ready | ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary | ErrorRecovery | Cancelling | Completed | Failed | Cancelled`
- `active_run`: `Option<RunId>`
- `primitive_kind`: `TurnPrimitiveKind`
- `admitted_content_shape`: `Option<ContentShape>`
- `vision_enabled`: `Bool`
- `image_tool_results_enabled`: `Bool`
- `tool_calls_pending`: `u32`
- `boundary_count`: `u32`
- `cancel_after_boundary`: `Bool`
- `terminal_outcome`: `TurnTerminalOutcome`

## Inputs
- `StartConversationRun`(run_id: RunId)
- `StartImmediateAppend`(run_id: RunId)
- `StartImmediateContext`(run_id: RunId)
- `PrimitiveApplied`(run_id: RunId, admitted_content_shape: ContentShape, vision_enabled: Bool, image_tool_results_enabled: Bool)
- `LlmReturnedToolCalls`(run_id: RunId, tool_count: u32)
- `LlmReturnedTerminal`(run_id: RunId)
- `ToolCallsResolved`(run_id: RunId)
- `BoundaryContinue`(run_id: RunId)
- `BoundaryComplete`(run_id: RunId)
- `RecoverableFailure`(run_id: RunId)
- `FatalFailure`(run_id: RunId)
- `RetryRequested`(run_id: RunId)
- `CancelNow`(run_id: RunId)
- `CancelAfterBoundary`(run_id: RunId)
- `CancellationObserved`(run_id: RunId)
- `AcknowledgeTerminal`(run_id: RunId)
- `RunCompleted`(run_id: RunId)
- `RunFailed`(run_id: RunId)
- `RunCancelled`(run_id: RunId)

## Effects
- `RunStarted`(run_id: RunId)
- `BoundaryApplied`(run_id: RunId, boundary_sequence: u64)
- `RunCompleted`(run_id: RunId)
- `RunFailed`(run_id: RunId)
- `RunCancelled`(run_id: RunId)

## Invariants
- `ready_has_no_active_run`
- `ready_has_no_admitted_content`
- `non_ready_has_active_run`
- `waiting_for_ops_implies_pending_tools`
- `ready_has_no_boundary_cancel_request`
- `immediate_primitives_skip_llm_and_recovery`
- `terminal_states_match_terminal_outcome`
- `completed_runs_have_seen_a_boundary`

## Transitions
### `StartConversationRun`
- From: `Ready`
- On: `StartConversationRun`(run_id)
- Emits: `RunStarted`
- To: `ApplyingPrimitive`

### `StartImmediateAppend`
- From: `Ready`
- On: `StartImmediateAppend`(run_id)
- Emits: `RunStarted`
- To: `ApplyingPrimitive`

### `StartImmediateContext`
- From: `Ready`
- On: `StartImmediateContext`(run_id)
- Emits: `RunStarted`
- To: `ApplyingPrimitive`

### `PrimitiveAppliedConversationTurn`
- From: `ApplyingPrimitive`
- On: `PrimitiveApplied`(run_id, admitted_content_shape, vision_enabled, image_tool_results_enabled)
- Guards:
  - `run_matches_active`
  - `conversation_turn`
- To: `CallingLlm`

### `PrimitiveAppliedImmediateAppend`
- From: `ApplyingPrimitive`
- On: `PrimitiveApplied`(run_id, admitted_content_shape, vision_enabled, image_tool_results_enabled)
- Guards:
  - `run_matches_active`
  - `immediate_append`
  - `cancel_after_boundary_not_requested`
- Emits: `BoundaryApplied`, `RunCompleted`
- To: `Completed`

### `PrimitiveAppliedImmediateAppendCancelsAfterBoundary`
- From: `ApplyingPrimitive`
- On: `PrimitiveApplied`(run_id, admitted_content_shape, vision_enabled, image_tool_results_enabled)
- Guards:
  - `run_matches_active`
  - `immediate_append`
  - `boundary_cancel_requested`
- Emits: `BoundaryApplied`, `RunCancelled`
- To: `Cancelled`

### `PrimitiveAppliedImmediateContext`
- From: `ApplyingPrimitive`
- On: `PrimitiveApplied`(run_id, admitted_content_shape, vision_enabled, image_tool_results_enabled)
- Guards:
  - `run_matches_active`
  - `immediate_context`
  - `cancel_after_boundary_not_requested`
- Emits: `BoundaryApplied`, `RunCompleted`
- To: `Completed`

### `PrimitiveAppliedImmediateContextCancelsAfterBoundary`
- From: `ApplyingPrimitive`
- On: `PrimitiveApplied`(run_id, admitted_content_shape, vision_enabled, image_tool_results_enabled)
- Guards:
  - `run_matches_active`
  - `immediate_context`
  - `boundary_cancel_requested`
- Emits: `BoundaryApplied`, `RunCancelled`
- To: `Cancelled`

### `LlmReturnedToolCalls`
- From: `CallingLlm`
- On: `LlmReturnedToolCalls`(run_id, tool_count)
- Guards:
  - `run_matches_active`
  - `tool_count_positive`
- To: `WaitingForOps`

### `ToolCallsResolved`
- From: `WaitingForOps`
- On: `ToolCallsResolved`(run_id)
- Guards:
  - `run_matches_active`
  - `tool_calls_pending_positive`
- Emits: `BoundaryApplied`
- To: `DrainingBoundary`

### `LlmReturnedTerminal`
- From: `CallingLlm`
- On: `LlmReturnedTerminal`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `BoundaryApplied`
- To: `DrainingBoundary`

### `BoundaryContinue`
- From: `DrainingBoundary`
- On: `BoundaryContinue`(run_id)
- Guards:
  - `run_matches_active`
  - `conversation_turn`
  - `cancel_after_boundary_not_requested`
- To: `CallingLlm`

### `BoundaryContinueCancelsAfterBoundary`
- From: `DrainingBoundary`
- On: `BoundaryContinue`(run_id)
- Guards:
  - `run_matches_active`
  - `conversation_turn`
  - `boundary_cancel_requested`
- Emits: `RunCancelled`
- To: `Cancelled`

### `BoundaryComplete`
- From: `DrainingBoundary`
- On: `BoundaryComplete`(run_id)
- Guards:
  - `run_matches_active`
  - `cancel_after_boundary_not_requested`
- Emits: `RunCompleted`
- To: `Completed`

### `BoundaryCompleteCancelsAfterBoundary`
- From: `DrainingBoundary`
- On: `BoundaryComplete`(run_id)
- Guards:
  - `run_matches_active`
  - `boundary_cancel_requested`
- Emits: `RunCancelled`
- To: `Cancelled`

### `RecoverableFailureFromCallingLlm`
- From: `CallingLlm`
- On: `RecoverableFailure`(run_id)
- Guards:
  - `run_matches_active`
- To: `ErrorRecovery`

### `RecoverableFailureFromWaitingForOps`
- From: `WaitingForOps`
- On: `RecoverableFailure`(run_id)
- Guards:
  - `run_matches_active`
- To: `ErrorRecovery`

### `RecoverableFailureFromDrainingBoundary`
- From: `DrainingBoundary`
- On: `RecoverableFailure`(run_id)
- Guards:
  - `run_matches_active`
- To: `ErrorRecovery`

### `RetryRequested`
- From: `ErrorRecovery`
- On: `RetryRequested`(run_id)
- Guards:
  - `run_matches_active`
- To: `CallingLlm`

### `FatalFailureFromApplyingPrimitive`
- From: `ApplyingPrimitive`
- On: `FatalFailure`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `RunFailed`
- To: `Failed`

### `FatalFailureFromCallingLlm`
- From: `CallingLlm`
- On: `FatalFailure`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `RunFailed`
- To: `Failed`

### `FatalFailureFromWaitingForOps`
- From: `WaitingForOps`
- On: `FatalFailure`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `RunFailed`
- To: `Failed`

### `FatalFailureFromDrainingBoundary`
- From: `DrainingBoundary`
- On: `FatalFailure`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `RunFailed`
- To: `Failed`

### `FatalFailureFromErrorRecovery`
- From: `ErrorRecovery`
- On: `FatalFailure`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `RunFailed`
- To: `Failed`

### `CancelNowFromApplyingPrimitive`
- From: `ApplyingPrimitive`
- On: `CancelNow`(run_id)
- Guards:
  - `run_matches_active`
- To: `Cancelling`

### `CancelNowFromCallingLlm`
- From: `CallingLlm`
- On: `CancelNow`(run_id)
- Guards:
  - `run_matches_active`
- To: `Cancelling`

### `CancelNowFromWaitingForOps`
- From: `WaitingForOps`
- On: `CancelNow`(run_id)
- Guards:
  - `run_matches_active`
- To: `Cancelling`

### `CancelNowFromDrainingBoundary`
- From: `DrainingBoundary`
- On: `CancelNow`(run_id)
- Guards:
  - `run_matches_active`
- To: `Cancelling`

### `CancelNowFromErrorRecovery`
- From: `ErrorRecovery`
- On: `CancelNow`(run_id)
- Guards:
  - `run_matches_active`
- To: `Cancelling`

### `CancelAfterBoundaryFromApplyingPrimitive`
- From: `ApplyingPrimitive`
- On: `CancelAfterBoundary`(run_id)
- Guards:
  - `run_matches_active`
- To: `ApplyingPrimitive`

### `CancelAfterBoundaryFromCallingLlm`
- From: `CallingLlm`
- On: `CancelAfterBoundary`(run_id)
- Guards:
  - `run_matches_active`
- To: `CallingLlm`

### `CancelAfterBoundaryFromWaitingForOps`
- From: `WaitingForOps`
- On: `CancelAfterBoundary`(run_id)
- Guards:
  - `run_matches_active`
- To: `WaitingForOps`

### `CancelAfterBoundaryFromDrainingBoundary`
- From: `DrainingBoundary`
- On: `CancelAfterBoundary`(run_id)
- Guards:
  - `run_matches_active`
- To: `DrainingBoundary`

### `CancelAfterBoundaryFromErrorRecovery`
- From: `ErrorRecovery`
- On: `CancelAfterBoundary`(run_id)
- Guards:
  - `run_matches_active`
- To: `ErrorRecovery`

### `CancellationObserved`
- From: `Cancelling`
- On: `CancellationObserved`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `RunCancelled`
- To: `Cancelled`

### `AcknowledgeTerminalFromCompleted`
- From: `Completed`
- On: `AcknowledgeTerminal`(run_id)
- Guards:
  - `run_matches_active`
- To: `Ready`

### `AcknowledgeTerminalFromFailed`
- From: `Failed`
- On: `AcknowledgeTerminal`(run_id)
- Guards:
  - `run_matches_active`
- To: `Ready`

### `AcknowledgeTerminalFromCancelled`
- From: `Cancelled`
- On: `AcknowledgeTerminal`(run_id)
- Guards:
  - `run_matches_active`
- To: `Ready`

## Coverage
### Code Anchors
- `meerkat-core/src/agent/state.rs` — core turn loop state precursor
- `meerkat-core/src/agent/runner.rs` — turn runner precursor
- `meerkat-core/src/lifecycle/run_primitive.rs` — canonical run primitive input precursor
- `meerkat-core/src/lifecycle/run_event.rs` — canonical run event/effect precursor

### Scenarios
- `conversation-run` — conversation run starts, applies boundaries, and completes cleanly
- `tool-and-retry-loop` — tool calls and retry/yield semantics stay inside the turn owner
- `cancel-and-fail` — cancelled and failed runs produce explicit terminal outcomes
