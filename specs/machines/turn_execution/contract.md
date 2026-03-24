# TurnExecutionMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-core` / `generated::turn_execution`

## State
- Phase enum: `Ready | ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary | Extracting | ErrorRecovery | Cancelling | Completed | Failed | Cancelled`
- `active_run`: `Option<RunId>`
- `primitive_kind`: `TurnPrimitiveKind`
- `admitted_content_shape`: `Option<ContentShape>`
- `vision_enabled`: `Bool`
- `image_tool_results_enabled`: `Bool`
- `tool_calls_pending`: `u32`
- `pending_op_refs`: `Option<Seq<AsyncOpRef>>`
- `barrier_operation_ids`: `Seq<OperationId>`
- `has_barrier_ops`: `Bool`
- `barrier_satisfied`: `Bool`
- `boundary_count`: `u32`
- `cancel_after_boundary`: `Bool`
- `terminal_outcome`: `TurnTerminalOutcome`
- `extraction_attempts`: `u32`
- `max_extraction_retries`: `u32`

## Inputs
- `StartConversationRun`(run_id: RunId)
- `StartImmediateAppend`(run_id: RunId)
- `StartImmediateContext`(run_id: RunId)
- `PrimitiveApplied`(run_id: RunId, admitted_content_shape: ContentShape, vision_enabled: Bool, image_tool_results_enabled: Bool)
- `LlmReturnedToolCalls`(run_id: RunId, tool_count: u32)
- `LlmReturnedTerminal`(run_id: RunId)
- `RegisterPendingOps`(run_id: RunId, op_refs: Seq<AsyncOpRef>, barrier_operation_ids: Seq<OperationId>, has_barrier_ops: Bool)
- `ToolCallsResolved`(run_id: RunId)
- `OpsBarrierSatisfied`(run_id: RunId, operation_ids: Seq<OperationId>)
- `BoundaryContinue`(run_id: RunId)
- `BoundaryComplete`(run_id: RunId)
- `RecoverableFailure`(run_id: RunId)
- `FatalFailure`(run_id: RunId)
- `RetryRequested`(run_id: RunId)
- `CancelNow`(run_id: RunId)
- `CancelAfterBoundary`(run_id: RunId)
- `CancellationObserved`(run_id: RunId)
- `AcknowledgeTerminal`(run_id: RunId)
- `TurnLimitReached`(run_id: RunId)
- `BudgetExhausted`(run_id: RunId)
- `TimeBudgetExceeded`(run_id: RunId)
- `EnterExtraction`(run_id: RunId, max_retries: u32)
- `ExtractionValidationPassed`(run_id: RunId)
- `ExtractionRetry`(run_id: RunId)
- `ExtractionExhausted`(run_id: RunId)
- `ForceCancelNoRun`
- `RunCompleted`(run_id: RunId)
- `RunFailed`(run_id: RunId)
- `RunCancelled`(run_id: RunId)

## Effects
- `RunStarted`(run_id: RunId)
- `BoundaryApplied`(run_id: RunId, boundary_sequence: u64)
- `RunCompleted`(run_id: RunId)
- `RunFailed`(run_id: RunId)
- `RunCancelled`(run_id: RunId)
- `CheckCompaction`

## Invariants
- `ready_has_no_active_run`
- `ready_has_no_admitted_content`
- `non_ready_has_active_run`
- `waiting_for_ops_implies_pending_tools`
- `pending_op_refs_only_used_while_waiting`
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
- Emits: `CheckCompaction`
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

### `RegisterPendingOps`
- From: `WaitingForOps`
- On: `RegisterPendingOps`(run_id, op_refs, barrier_operation_ids, has_barrier_ops)
- Guards:
  - `run_matches_active`
  - `tool_calls_pending_positive`
- To: `WaitingForOps`

### `OpsBarrierSatisfied`
- From: `WaitingForOps`
- On: `OpsBarrierSatisfied`(run_id, operation_ids)
- Guards:
  - `run_matches_active`
  - `barrier_not_yet_satisfied`
  - `operation_ids_match_current_barrier_set`
- To: `WaitingForOps`

### `ToolCallsResolved`
- From: `WaitingForOps`
- On: `ToolCallsResolved`(run_id)
- Guards:
  - `run_matches_active`
  - `tool_calls_pending_positive`
  - `pending_op_refs_registered`
  - `barrier_satisfied`
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
- Emits: `CheckCompaction`
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

### `EnterExtraction`
- From: `DrainingBoundary`
- On: `EnterExtraction`(run_id, max_retries)
- Guards:
  - `run_matches_active`
- To: `Extracting`

### `ExtractionValidationPassed`
- From: `Extracting`
- On: `ExtractionValidationPassed`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `RunCompleted`
- To: `Completed`

### `ExtractionRetry`
- From: `Extracting`
- On: `ExtractionRetry`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `CheckCompaction`
- To: `CallingLlm`

### `ExtractionExhausted`
- From: `Extracting`
- On: `ExtractionExhausted`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `RunCompleted`
- To: `Completed`

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
- Emits: `CheckCompaction`
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

### `FatalFailureFromExtracting`
- From: `Extracting`
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

### `CancelNowFromExtracting`
- From: `Extracting`
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

### `CancelAfterBoundaryFromExtracting`
- From: `Extracting`
- On: `CancelAfterBoundary`(run_id)
- Guards:
  - `run_matches_active`
- To: `Extracting`

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

### `TurnLimitReachedFromApplyingPrimitive`
- From: `ApplyingPrimitive`
- On: `TurnLimitReached`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `BoundaryApplied`, `RunCompleted`
- To: `Completed`

### `TurnLimitReachedFromCallingLlm`
- From: `CallingLlm`
- On: `TurnLimitReached`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `BoundaryApplied`, `RunCompleted`
- To: `Completed`

### `TurnLimitReachedFromWaitingForOps`
- From: `WaitingForOps`
- On: `TurnLimitReached`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `BoundaryApplied`, `RunCompleted`
- To: `Completed`

### `TurnLimitReachedFromDrainingBoundary`
- From: `DrainingBoundary`
- On: `TurnLimitReached`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `BoundaryApplied`, `RunCompleted`
- To: `Completed`

### `TurnLimitReachedFromExtracting`
- From: `Extracting`
- On: `TurnLimitReached`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `BoundaryApplied`, `RunCompleted`
- To: `Completed`

### `TurnLimitReachedFromErrorRecovery`
- From: `ErrorRecovery`
- On: `TurnLimitReached`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `BoundaryApplied`, `RunCompleted`
- To: `Completed`

### `BudgetExhaustedFromApplyingPrimitive`
- From: `ApplyingPrimitive`
- On: `BudgetExhausted`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `BoundaryApplied`, `RunCompleted`
- To: `Completed`

### `BudgetExhaustedFromCallingLlm`
- From: `CallingLlm`
- On: `BudgetExhausted`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `BoundaryApplied`, `RunCompleted`
- To: `Completed`

### `BudgetExhaustedFromWaitingForOps`
- From: `WaitingForOps`
- On: `BudgetExhausted`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `BoundaryApplied`, `RunCompleted`
- To: `Completed`

### `BudgetExhaustedFromDrainingBoundary`
- From: `DrainingBoundary`
- On: `BudgetExhausted`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `BoundaryApplied`, `RunCompleted`
- To: `Completed`

### `BudgetExhaustedFromExtracting`
- From: `Extracting`
- On: `BudgetExhausted`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `BoundaryApplied`, `RunCompleted`
- To: `Completed`

### `BudgetExhaustedFromErrorRecovery`
- From: `ErrorRecovery`
- On: `BudgetExhausted`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `BoundaryApplied`, `RunCompleted`
- To: `Completed`

### `TimeBudgetExceededFromApplyingPrimitive`
- From: `ApplyingPrimitive`
- On: `TimeBudgetExceeded`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `BoundaryApplied`, `RunCompleted`
- To: `Completed`

### `TimeBudgetExceededFromCallingLlm`
- From: `CallingLlm`
- On: `TimeBudgetExceeded`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `BoundaryApplied`, `RunCompleted`
- To: `Completed`

### `TimeBudgetExceededFromWaitingForOps`
- From: `WaitingForOps`
- On: `TimeBudgetExceeded`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `BoundaryApplied`, `RunCompleted`
- To: `Completed`

### `TimeBudgetExceededFromDrainingBoundary`
- From: `DrainingBoundary`
- On: `TimeBudgetExceeded`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `BoundaryApplied`, `RunCompleted`
- To: `Completed`

### `TimeBudgetExceededFromExtracting`
- From: `Extracting`
- On: `TimeBudgetExceeded`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `BoundaryApplied`, `RunCompleted`
- To: `Completed`

### `TimeBudgetExceededFromErrorRecovery`
- From: `ErrorRecovery`
- On: `TimeBudgetExceeded`(run_id)
- Guards:
  - `run_matches_active`
- Emits: `BoundaryApplied`, `RunCompleted`
- To: `Completed`

### `ForceCancelNoRunFromReady`
- From: `Ready`
- On: `ForceCancelNoRun`()
- To: `Cancelled`

### `ForceCancelNoRunFromApplyingPrimitive`
- From: `ApplyingPrimitive`
- On: `ForceCancelNoRun`()
- To: `Cancelled`

### `ForceCancelNoRunFromCallingLlm`
- From: `CallingLlm`
- On: `ForceCancelNoRun`()
- To: `Cancelled`

### `ForceCancelNoRunFromWaitingForOps`
- From: `WaitingForOps`
- On: `ForceCancelNoRun`()
- To: `Cancelled`

### `ForceCancelNoRunFromDrainingBoundary`
- From: `DrainingBoundary`
- On: `ForceCancelNoRun`()
- To: `Cancelled`

### `ForceCancelNoRunFromExtracting`
- From: `Extracting`
- On: `ForceCancelNoRun`()
- To: `Cancelled`

### `ForceCancelNoRunFromErrorRecovery`
- From: `ErrorRecovery`
- On: `ForceCancelNoRun`()
- To: `Cancelled`

### `ForceCancelNoRunFromCancelling`
- From: `Cancelling`
- On: `ForceCancelNoRun`()
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
