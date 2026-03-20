---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for TurnExecutionMachine.

CONSTANTS BooleanValues, ContentShapeValues, NatValues, OperationIdValues, RunIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

SeqOfOperationIdValues == {<<>>} \cup {<<x>> : x \in OperationIdValues} \cup {<<x, y>> : x \in OperationIdValues, y \in OperationIdValues}

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, terminal_outcome, extraction_attempts, max_extraction_retries

vars == << phase, model_step_count, active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, terminal_outcome, extraction_attempts, max_extraction_retries >>

Init ==
    /\ phase = "Ready"
    /\ model_step_count = 0
    /\ active_run = None
    /\ primitive_kind = "None"
    /\ admitted_content_shape = None
    /\ vision_enabled = FALSE
    /\ image_tool_results_enabled = FALSE
    /\ tool_calls_pending = 0
    /\ pending_op_ids = None
    /\ boundary_count = 0
    /\ cancel_after_boundary = FALSE
    /\ terminal_outcome = "None"
    /\ extraction_attempts = 0
    /\ max_extraction_retries = 0

TerminalStutter ==
    /\ phase = "Completed" \/ phase = "Failed" \/ phase = "Cancelled"
    /\ UNCHANGED vars

StartConversationRun(run_id) ==
    /\ phase = "Ready"
    /\ phase' = "ApplyingPrimitive"
    /\ model_step_count' = model_step_count + 1
    /\ active_run' = Some(run_id)
    /\ primitive_kind' = "ConversationTurn"
    /\ admitted_content_shape' = None
    /\ vision_enabled' = FALSE
    /\ image_tool_results_enabled' = FALSE
    /\ tool_calls_pending' = 0
    /\ boundary_count' = 0
    /\ cancel_after_boundary' = FALSE
    /\ terminal_outcome' = "None"
    /\ extraction_attempts' = 0
    /\ max_extraction_retries' = 0
    /\ UNCHANGED << pending_op_ids >>


StartImmediateAppend(run_id) ==
    /\ phase = "Ready"
    /\ phase' = "ApplyingPrimitive"
    /\ model_step_count' = model_step_count + 1
    /\ active_run' = Some(run_id)
    /\ primitive_kind' = "ImmediateAppend"
    /\ admitted_content_shape' = None
    /\ vision_enabled' = FALSE
    /\ image_tool_results_enabled' = FALSE
    /\ tool_calls_pending' = 0
    /\ boundary_count' = 0
    /\ cancel_after_boundary' = FALSE
    /\ terminal_outcome' = "None"
    /\ extraction_attempts' = 0
    /\ max_extraction_retries' = 0
    /\ UNCHANGED << pending_op_ids >>


StartImmediateContext(run_id) ==
    /\ phase = "Ready"
    /\ phase' = "ApplyingPrimitive"
    /\ model_step_count' = model_step_count + 1
    /\ active_run' = Some(run_id)
    /\ primitive_kind' = "ImmediateContextAppend"
    /\ admitted_content_shape' = None
    /\ vision_enabled' = FALSE
    /\ image_tool_results_enabled' = FALSE
    /\ tool_calls_pending' = 0
    /\ boundary_count' = 0
    /\ cancel_after_boundary' = FALSE
    /\ terminal_outcome' = "None"
    /\ extraction_attempts' = 0
    /\ max_extraction_retries' = 0
    /\ UNCHANGED << pending_op_ids >>


PrimitiveAppliedConversationTurn(run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled) ==
    /\ phase = "ApplyingPrimitive"
    /\ (active_run = Some(run_id))
    /\ (primitive_kind = "ConversationTurn")
    /\ phase' = "CallingLlm"
    /\ model_step_count' = model_step_count + 1
    /\ admitted_content_shape' = Some(arg_admitted_content_shape)
    /\ vision_enabled' = arg_vision_enabled
    /\ image_tool_results_enabled' = arg_image_tool_results_enabled
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, terminal_outcome, extraction_attempts, max_extraction_retries >>


PrimitiveAppliedImmediateAppend(run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled) ==
    /\ phase = "ApplyingPrimitive"
    /\ (active_run = Some(run_id))
    /\ (primitive_kind = "ImmediateAppend")
    /\ (cancel_after_boundary = FALSE)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ admitted_content_shape' = Some(arg_admitted_content_shape)
    /\ vision_enabled' = arg_vision_enabled
    /\ image_tool_results_enabled' = arg_image_tool_results_enabled
    /\ boundary_count' = (boundary_count) + 1
    /\ terminal_outcome' = "Completed"
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, pending_op_ids, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


PrimitiveAppliedImmediateAppendCancelsAfterBoundary(run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled) ==
    /\ phase = "ApplyingPrimitive"
    /\ (active_run = Some(run_id))
    /\ (primitive_kind = "ImmediateAppend")
    /\ (cancel_after_boundary = TRUE)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ admitted_content_shape' = Some(arg_admitted_content_shape)
    /\ vision_enabled' = arg_vision_enabled
    /\ image_tool_results_enabled' = arg_image_tool_results_enabled
    /\ boundary_count' = (boundary_count) + 1
    /\ cancel_after_boundary' = FALSE
    /\ terminal_outcome' = "Cancelled"
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, pending_op_ids, extraction_attempts, max_extraction_retries >>


PrimitiveAppliedImmediateContext(run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled) ==
    /\ phase = "ApplyingPrimitive"
    /\ (active_run = Some(run_id))
    /\ (primitive_kind = "ImmediateContextAppend")
    /\ (cancel_after_boundary = FALSE)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ admitted_content_shape' = Some(arg_admitted_content_shape)
    /\ vision_enabled' = arg_vision_enabled
    /\ image_tool_results_enabled' = arg_image_tool_results_enabled
    /\ boundary_count' = (boundary_count) + 1
    /\ terminal_outcome' = "Completed"
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, pending_op_ids, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


PrimitiveAppliedImmediateContextCancelsAfterBoundary(run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled) ==
    /\ phase = "ApplyingPrimitive"
    /\ (active_run = Some(run_id))
    /\ (primitive_kind = "ImmediateContextAppend")
    /\ (cancel_after_boundary = TRUE)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ admitted_content_shape' = Some(arg_admitted_content_shape)
    /\ vision_enabled' = arg_vision_enabled
    /\ image_tool_results_enabled' = arg_image_tool_results_enabled
    /\ boundary_count' = (boundary_count) + 1
    /\ cancel_after_boundary' = FALSE
    /\ terminal_outcome' = "Cancelled"
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, pending_op_ids, extraction_attempts, max_extraction_retries >>


LlmReturnedToolCalls(run_id, tool_count) ==
    /\ phase = "CallingLlm"
    /\ (active_run = Some(run_id))
    /\ (tool_count > 0)
    /\ phase' = "WaitingForOps"
    /\ model_step_count' = model_step_count + 1
    /\ tool_calls_pending' = tool_count
    /\ pending_op_ids' = None
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, boundary_count, cancel_after_boundary, terminal_outcome, extraction_attempts, max_extraction_retries >>


RegisterPendingOps(run_id, operation_ids) ==
    /\ phase = "WaitingForOps"
    /\ (active_run = Some(run_id))
    /\ (tool_calls_pending > 0)
    /\ phase' = "WaitingForOps"
    /\ model_step_count' = model_step_count + 1
    /\ pending_op_ids' = Some(operation_ids)
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, boundary_count, cancel_after_boundary, terminal_outcome, extraction_attempts, max_extraction_retries >>


ToolCallsResolved(run_id) ==
    /\ phase = "WaitingForOps"
    /\ (active_run = Some(run_id))
    /\ (tool_calls_pending > 0)
    /\ (pending_op_ids # None)
    /\ phase' = "DrainingBoundary"
    /\ model_step_count' = model_step_count + 1
    /\ tool_calls_pending' = 0
    /\ pending_op_ids' = None
    /\ boundary_count' = (boundary_count) + 1
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, cancel_after_boundary, terminal_outcome, extraction_attempts, max_extraction_retries >>


LlmReturnedTerminal(run_id) ==
    /\ phase = "CallingLlm"
    /\ (active_run = Some(run_id))
    /\ phase' = "DrainingBoundary"
    /\ model_step_count' = model_step_count + 1
    /\ boundary_count' = (boundary_count) + 1
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, cancel_after_boundary, terminal_outcome, extraction_attempts, max_extraction_retries >>


BoundaryContinue(run_id) ==
    /\ phase = "DrainingBoundary"
    /\ (active_run = Some(run_id))
    /\ (primitive_kind = "ConversationTurn")
    /\ (cancel_after_boundary = FALSE)
    /\ phase' = "CallingLlm"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, terminal_outcome, extraction_attempts, max_extraction_retries >>


BoundaryContinueCancelsAfterBoundary(run_id) ==
    /\ phase = "DrainingBoundary"
    /\ (active_run = Some(run_id))
    /\ (primitive_kind = "ConversationTurn")
    /\ (cancel_after_boundary = TRUE)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ cancel_after_boundary' = FALSE
    /\ terminal_outcome' = "Cancelled"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, extraction_attempts, max_extraction_retries >>


BoundaryComplete(run_id) ==
    /\ phase = "DrainingBoundary"
    /\ (active_run = Some(run_id))
    /\ (cancel_after_boundary = FALSE)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Completed"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


BoundaryCompleteCancelsAfterBoundary(run_id) ==
    /\ phase = "DrainingBoundary"
    /\ (active_run = Some(run_id))
    /\ (cancel_after_boundary = TRUE)
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ cancel_after_boundary' = FALSE
    /\ terminal_outcome' = "Cancelled"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, extraction_attempts, max_extraction_retries >>


EnterExtraction(run_id, max_retries) ==
    /\ phase = "DrainingBoundary"
    /\ (active_run = Some(run_id))
    /\ phase' = "Extracting"
    /\ model_step_count' = model_step_count + 1
    /\ max_extraction_retries' = max_retries
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, terminal_outcome, extraction_attempts >>


ExtractionValidationPassed(run_id) ==
    /\ phase = "Extracting"
    /\ (active_run = Some(run_id))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Completed"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


ExtractionRetry(run_id) ==
    /\ phase = "Extracting"
    /\ (active_run = Some(run_id))
    /\ phase' = "CallingLlm"
    /\ model_step_count' = model_step_count + 1
    /\ extraction_attempts' = (extraction_attempts) + 1
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, terminal_outcome, max_extraction_retries >>


ExtractionExhausted(run_id) ==
    /\ phase = "Extracting"
    /\ (active_run = Some(run_id))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Completed"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


RecoverableFailureFromCallingLlm(run_id) ==
    /\ phase = "CallingLlm"
    /\ (active_run = Some(run_id))
    /\ phase' = "ErrorRecovery"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, terminal_outcome, extraction_attempts, max_extraction_retries >>


RecoverableFailureFromWaitingForOps(run_id) ==
    /\ phase = "WaitingForOps"
    /\ (active_run = Some(run_id))
    /\ phase' = "ErrorRecovery"
    /\ model_step_count' = model_step_count + 1
    /\ pending_op_ids' = None
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, boundary_count, cancel_after_boundary, terminal_outcome, extraction_attempts, max_extraction_retries >>


RecoverableFailureFromDrainingBoundary(run_id) ==
    /\ phase = "DrainingBoundary"
    /\ (active_run = Some(run_id))
    /\ phase' = "ErrorRecovery"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, terminal_outcome, extraction_attempts, max_extraction_retries >>


RetryRequested(run_id) ==
    /\ phase = "ErrorRecovery"
    /\ (active_run = Some(run_id))
    /\ phase' = "CallingLlm"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, terminal_outcome, extraction_attempts, max_extraction_retries >>


FatalFailureFromApplyingPrimitive(run_id) ==
    /\ phase = "ApplyingPrimitive"
    /\ (active_run = Some(run_id))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Failed"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


FatalFailureFromCallingLlm(run_id) ==
    /\ phase = "CallingLlm"
    /\ (active_run = Some(run_id))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Failed"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


FatalFailureFromWaitingForOps(run_id) ==
    /\ phase = "WaitingForOps"
    /\ (active_run = Some(run_id))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ pending_op_ids' = None
    /\ terminal_outcome' = "Failed"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, boundary_count, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


FatalFailureFromDrainingBoundary(run_id) ==
    /\ phase = "DrainingBoundary"
    /\ (active_run = Some(run_id))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Failed"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


FatalFailureFromExtracting(run_id) ==
    /\ phase = "Extracting"
    /\ (active_run = Some(run_id))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Failed"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


FatalFailureFromErrorRecovery(run_id) ==
    /\ phase = "ErrorRecovery"
    /\ (active_run = Some(run_id))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Failed"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


CancelNowFromApplyingPrimitive(run_id) ==
    /\ phase = "ApplyingPrimitive"
    /\ (active_run = Some(run_id))
    /\ phase' = "Cancelling"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, terminal_outcome, extraction_attempts, max_extraction_retries >>


CancelNowFromCallingLlm(run_id) ==
    /\ phase = "CallingLlm"
    /\ (active_run = Some(run_id))
    /\ phase' = "Cancelling"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, terminal_outcome, extraction_attempts, max_extraction_retries >>


CancelNowFromWaitingForOps(run_id) ==
    /\ phase = "WaitingForOps"
    /\ (active_run = Some(run_id))
    /\ phase' = "Cancelling"
    /\ model_step_count' = model_step_count + 1
    /\ pending_op_ids' = None
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, boundary_count, cancel_after_boundary, terminal_outcome, extraction_attempts, max_extraction_retries >>


CancelNowFromDrainingBoundary(run_id) ==
    /\ phase = "DrainingBoundary"
    /\ (active_run = Some(run_id))
    /\ phase' = "Cancelling"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, terminal_outcome, extraction_attempts, max_extraction_retries >>


CancelNowFromExtracting(run_id) ==
    /\ phase = "Extracting"
    /\ (active_run = Some(run_id))
    /\ phase' = "Cancelling"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, terminal_outcome, extraction_attempts, max_extraction_retries >>


CancelNowFromErrorRecovery(run_id) ==
    /\ phase = "ErrorRecovery"
    /\ (active_run = Some(run_id))
    /\ phase' = "Cancelling"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, terminal_outcome, extraction_attempts, max_extraction_retries >>


CancelAfterBoundaryFromApplyingPrimitive(run_id) ==
    /\ phase = "ApplyingPrimitive"
    /\ (active_run = Some(run_id))
    /\ phase' = "ApplyingPrimitive"
    /\ model_step_count' = model_step_count + 1
    /\ cancel_after_boundary' = TRUE
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, terminal_outcome, extraction_attempts, max_extraction_retries >>


CancelAfterBoundaryFromCallingLlm(run_id) ==
    /\ phase = "CallingLlm"
    /\ (active_run = Some(run_id))
    /\ phase' = "CallingLlm"
    /\ model_step_count' = model_step_count + 1
    /\ cancel_after_boundary' = TRUE
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, terminal_outcome, extraction_attempts, max_extraction_retries >>


CancelAfterBoundaryFromWaitingForOps(run_id) ==
    /\ phase = "WaitingForOps"
    /\ (active_run = Some(run_id))
    /\ phase' = "WaitingForOps"
    /\ model_step_count' = model_step_count + 1
    /\ cancel_after_boundary' = TRUE
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, terminal_outcome, extraction_attempts, max_extraction_retries >>


CancelAfterBoundaryFromDrainingBoundary(run_id) ==
    /\ phase = "DrainingBoundary"
    /\ (active_run = Some(run_id))
    /\ phase' = "DrainingBoundary"
    /\ model_step_count' = model_step_count + 1
    /\ cancel_after_boundary' = TRUE
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, terminal_outcome, extraction_attempts, max_extraction_retries >>


CancelAfterBoundaryFromExtracting(run_id) ==
    /\ phase = "Extracting"
    /\ (active_run = Some(run_id))
    /\ phase' = "Extracting"
    /\ model_step_count' = model_step_count + 1
    /\ cancel_after_boundary' = TRUE
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, terminal_outcome, extraction_attempts, max_extraction_retries >>


CancelAfterBoundaryFromErrorRecovery(run_id) ==
    /\ phase = "ErrorRecovery"
    /\ (active_run = Some(run_id))
    /\ phase' = "ErrorRecovery"
    /\ model_step_count' = model_step_count + 1
    /\ cancel_after_boundary' = TRUE
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, terminal_outcome, extraction_attempts, max_extraction_retries >>


CancellationObserved(run_id) ==
    /\ phase = "Cancelling"
    /\ (active_run = Some(run_id))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ cancel_after_boundary' = FALSE
    /\ terminal_outcome' = "Cancelled"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, extraction_attempts, max_extraction_retries >>


TurnLimitReachedFromApplyingPrimitive(run_id) ==
    /\ phase = "ApplyingPrimitive"
    /\ (active_run = Some(run_id))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ boundary_count' = (boundary_count) + 1
    /\ terminal_outcome' = "Completed"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


TurnLimitReachedFromCallingLlm(run_id) ==
    /\ phase = "CallingLlm"
    /\ (active_run = Some(run_id))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ boundary_count' = (boundary_count) + 1
    /\ terminal_outcome' = "Completed"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


TurnLimitReachedFromWaitingForOps(run_id) ==
    /\ phase = "WaitingForOps"
    /\ (active_run = Some(run_id))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ pending_op_ids' = None
    /\ boundary_count' = (boundary_count) + 1
    /\ terminal_outcome' = "Completed"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


TurnLimitReachedFromDrainingBoundary(run_id) ==
    /\ phase = "DrainingBoundary"
    /\ (active_run = Some(run_id))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ boundary_count' = (boundary_count) + 1
    /\ terminal_outcome' = "Completed"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


TurnLimitReachedFromExtracting(run_id) ==
    /\ phase = "Extracting"
    /\ (active_run = Some(run_id))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ boundary_count' = (boundary_count) + 1
    /\ terminal_outcome' = "Completed"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


TurnLimitReachedFromErrorRecovery(run_id) ==
    /\ phase = "ErrorRecovery"
    /\ (active_run = Some(run_id))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ boundary_count' = (boundary_count) + 1
    /\ terminal_outcome' = "Completed"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


BudgetExhaustedFromApplyingPrimitive(run_id) ==
    /\ phase = "ApplyingPrimitive"
    /\ (active_run = Some(run_id))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ boundary_count' = (boundary_count) + 1
    /\ terminal_outcome' = "BudgetExhausted"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


BudgetExhaustedFromCallingLlm(run_id) ==
    /\ phase = "CallingLlm"
    /\ (active_run = Some(run_id))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ boundary_count' = (boundary_count) + 1
    /\ terminal_outcome' = "BudgetExhausted"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


BudgetExhaustedFromWaitingForOps(run_id) ==
    /\ phase = "WaitingForOps"
    /\ (active_run = Some(run_id))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ pending_op_ids' = None
    /\ boundary_count' = (boundary_count) + 1
    /\ terminal_outcome' = "BudgetExhausted"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


BudgetExhaustedFromDrainingBoundary(run_id) ==
    /\ phase = "DrainingBoundary"
    /\ (active_run = Some(run_id))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ boundary_count' = (boundary_count) + 1
    /\ terminal_outcome' = "BudgetExhausted"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


BudgetExhaustedFromExtracting(run_id) ==
    /\ phase = "Extracting"
    /\ (active_run = Some(run_id))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ boundary_count' = (boundary_count) + 1
    /\ terminal_outcome' = "BudgetExhausted"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


BudgetExhaustedFromErrorRecovery(run_id) ==
    /\ phase = "ErrorRecovery"
    /\ (active_run = Some(run_id))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ boundary_count' = (boundary_count) + 1
    /\ terminal_outcome' = "BudgetExhausted"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


ForceCancelNoRunFromReady ==
    /\ phase = "Ready"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Cancelled"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


ForceCancelNoRunFromApplyingPrimitive ==
    /\ phase = "ApplyingPrimitive"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Cancelled"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


ForceCancelNoRunFromCallingLlm ==
    /\ phase = "CallingLlm"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Cancelled"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


ForceCancelNoRunFromWaitingForOps ==
    /\ phase = "WaitingForOps"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ pending_op_ids' = None
    /\ terminal_outcome' = "Cancelled"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, boundary_count, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


ForceCancelNoRunFromDrainingBoundary ==
    /\ phase = "DrainingBoundary"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Cancelled"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


ForceCancelNoRunFromExtracting ==
    /\ phase = "Extracting"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Cancelled"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


ForceCancelNoRunFromErrorRecovery ==
    /\ phase = "ErrorRecovery"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Cancelled"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


ForceCancelNoRunFromCancelling ==
    /\ phase = "Cancelling"
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Cancelled"
    /\ UNCHANGED << active_run, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, tool_calls_pending, pending_op_ids, boundary_count, cancel_after_boundary, extraction_attempts, max_extraction_retries >>


AcknowledgeTerminalFromCompleted(run_id) ==
    /\ phase = "Completed"
    /\ (active_run = Some(run_id))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ active_run' = None
    /\ primitive_kind' = "None"
    /\ admitted_content_shape' = None
    /\ vision_enabled' = FALSE
    /\ image_tool_results_enabled' = FALSE
    /\ tool_calls_pending' = 0
    /\ boundary_count' = 0
    /\ cancel_after_boundary' = FALSE
    /\ terminal_outcome' = "None"
    /\ extraction_attempts' = 0
    /\ max_extraction_retries' = 0
    /\ UNCHANGED << pending_op_ids >>


AcknowledgeTerminalFromFailed(run_id) ==
    /\ phase = "Failed"
    /\ (active_run = Some(run_id))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ active_run' = None
    /\ primitive_kind' = "None"
    /\ admitted_content_shape' = None
    /\ vision_enabled' = FALSE
    /\ image_tool_results_enabled' = FALSE
    /\ tool_calls_pending' = 0
    /\ boundary_count' = 0
    /\ cancel_after_boundary' = FALSE
    /\ terminal_outcome' = "None"
    /\ extraction_attempts' = 0
    /\ max_extraction_retries' = 0
    /\ UNCHANGED << pending_op_ids >>


AcknowledgeTerminalFromCancelled(run_id) ==
    /\ phase = "Cancelled"
    /\ (active_run = Some(run_id))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ active_run' = None
    /\ primitive_kind' = "None"
    /\ admitted_content_shape' = None
    /\ vision_enabled' = FALSE
    /\ image_tool_results_enabled' = FALSE
    /\ tool_calls_pending' = 0
    /\ boundary_count' = 0
    /\ cancel_after_boundary' = FALSE
    /\ terminal_outcome' = "None"
    /\ extraction_attempts' = 0
    /\ max_extraction_retries' = 0
    /\ UNCHANGED << pending_op_ids >>


Next ==
    \/ \E run_id \in RunIdValues : StartConversationRun(run_id)
    \/ \E run_id \in RunIdValues : StartImmediateAppend(run_id)
    \/ \E run_id \in RunIdValues : StartImmediateContext(run_id)
    \/ \E run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : PrimitiveAppliedConversationTurn(run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)
    \/ \E run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : PrimitiveAppliedImmediateAppend(run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)
    \/ \E run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : PrimitiveAppliedImmediateAppendCancelsAfterBoundary(run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)
    \/ \E run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : PrimitiveAppliedImmediateContext(run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)
    \/ \E run_id \in RunIdValues : \E arg_admitted_content_shape \in ContentShapeValues : \E arg_vision_enabled \in BOOLEAN : \E arg_image_tool_results_enabled \in BOOLEAN : PrimitiveAppliedImmediateContextCancelsAfterBoundary(run_id, arg_admitted_content_shape, arg_vision_enabled, arg_image_tool_results_enabled)
    \/ \E run_id \in RunIdValues : \E tool_count \in 0..2 : LlmReturnedToolCalls(run_id, tool_count)
    \/ \E run_id \in RunIdValues : \E operation_ids \in SeqOfOperationIdValues : RegisterPendingOps(run_id, operation_ids)
    \/ \E run_id \in RunIdValues : ToolCallsResolved(run_id)
    \/ \E run_id \in RunIdValues : LlmReturnedTerminal(run_id)
    \/ \E run_id \in RunIdValues : BoundaryContinue(run_id)
    \/ \E run_id \in RunIdValues : BoundaryContinueCancelsAfterBoundary(run_id)
    \/ \E run_id \in RunIdValues : BoundaryComplete(run_id)
    \/ \E run_id \in RunIdValues : BoundaryCompleteCancelsAfterBoundary(run_id)
    \/ \E run_id \in RunIdValues : \E max_retries \in 0..2 : EnterExtraction(run_id, max_retries)
    \/ \E run_id \in RunIdValues : ExtractionValidationPassed(run_id)
    \/ \E run_id \in RunIdValues : ExtractionRetry(run_id)
    \/ \E run_id \in RunIdValues : ExtractionExhausted(run_id)
    \/ \E run_id \in RunIdValues : RecoverableFailureFromCallingLlm(run_id)
    \/ \E run_id \in RunIdValues : RecoverableFailureFromWaitingForOps(run_id)
    \/ \E run_id \in RunIdValues : RecoverableFailureFromDrainingBoundary(run_id)
    \/ \E run_id \in RunIdValues : RetryRequested(run_id)
    \/ \E run_id \in RunIdValues : FatalFailureFromApplyingPrimitive(run_id)
    \/ \E run_id \in RunIdValues : FatalFailureFromCallingLlm(run_id)
    \/ \E run_id \in RunIdValues : FatalFailureFromWaitingForOps(run_id)
    \/ \E run_id \in RunIdValues : FatalFailureFromDrainingBoundary(run_id)
    \/ \E run_id \in RunIdValues : FatalFailureFromExtracting(run_id)
    \/ \E run_id \in RunIdValues : FatalFailureFromErrorRecovery(run_id)
    \/ \E run_id \in RunIdValues : CancelNowFromApplyingPrimitive(run_id)
    \/ \E run_id \in RunIdValues : CancelNowFromCallingLlm(run_id)
    \/ \E run_id \in RunIdValues : CancelNowFromWaitingForOps(run_id)
    \/ \E run_id \in RunIdValues : CancelNowFromDrainingBoundary(run_id)
    \/ \E run_id \in RunIdValues : CancelNowFromExtracting(run_id)
    \/ \E run_id \in RunIdValues : CancelNowFromErrorRecovery(run_id)
    \/ \E run_id \in RunIdValues : CancelAfterBoundaryFromApplyingPrimitive(run_id)
    \/ \E run_id \in RunIdValues : CancelAfterBoundaryFromCallingLlm(run_id)
    \/ \E run_id \in RunIdValues : CancelAfterBoundaryFromWaitingForOps(run_id)
    \/ \E run_id \in RunIdValues : CancelAfterBoundaryFromDrainingBoundary(run_id)
    \/ \E run_id \in RunIdValues : CancelAfterBoundaryFromExtracting(run_id)
    \/ \E run_id \in RunIdValues : CancelAfterBoundaryFromErrorRecovery(run_id)
    \/ \E run_id \in RunIdValues : CancellationObserved(run_id)
    \/ \E run_id \in RunIdValues : TurnLimitReachedFromApplyingPrimitive(run_id)
    \/ \E run_id \in RunIdValues : TurnLimitReachedFromCallingLlm(run_id)
    \/ \E run_id \in RunIdValues : TurnLimitReachedFromWaitingForOps(run_id)
    \/ \E run_id \in RunIdValues : TurnLimitReachedFromDrainingBoundary(run_id)
    \/ \E run_id \in RunIdValues : TurnLimitReachedFromExtracting(run_id)
    \/ \E run_id \in RunIdValues : TurnLimitReachedFromErrorRecovery(run_id)
    \/ \E run_id \in RunIdValues : BudgetExhaustedFromApplyingPrimitive(run_id)
    \/ \E run_id \in RunIdValues : BudgetExhaustedFromCallingLlm(run_id)
    \/ \E run_id \in RunIdValues : BudgetExhaustedFromWaitingForOps(run_id)
    \/ \E run_id \in RunIdValues : BudgetExhaustedFromDrainingBoundary(run_id)
    \/ \E run_id \in RunIdValues : BudgetExhaustedFromExtracting(run_id)
    \/ \E run_id \in RunIdValues : BudgetExhaustedFromErrorRecovery(run_id)
    \/ ForceCancelNoRunFromReady
    \/ ForceCancelNoRunFromApplyingPrimitive
    \/ ForceCancelNoRunFromCallingLlm
    \/ ForceCancelNoRunFromWaitingForOps
    \/ ForceCancelNoRunFromDrainingBoundary
    \/ ForceCancelNoRunFromExtracting
    \/ ForceCancelNoRunFromErrorRecovery
    \/ ForceCancelNoRunFromCancelling
    \/ \E run_id \in RunIdValues : AcknowledgeTerminalFromCompleted(run_id)
    \/ \E run_id \in RunIdValues : AcknowledgeTerminalFromFailed(run_id)
    \/ \E run_id \in RunIdValues : AcknowledgeTerminalFromCancelled(run_id)
    \/ TerminalStutter

ready_has_no_active_run == ((phase # "Ready") \/ (active_run = None))
ready_has_no_admitted_content == ((phase # "Ready") \/ (admitted_content_shape = None))
non_ready_has_active_run == ((phase = "Ready") \/ (phase = "Completed") \/ (phase = "Failed") \/ (phase = "Cancelled") \/ (active_run # None))
waiting_for_ops_implies_pending_tools == ((phase # "WaitingForOps") \/ (tool_calls_pending > 0))
pending_op_ids_only_used_while_waiting == ((phase = "WaitingForOps") \/ (pending_op_ids = None))
ready_has_no_boundary_cancel_request == ((phase # "Ready") \/ (cancel_after_boundary = FALSE))
immediate_primitives_skip_llm_and_recovery == ((primitive_kind = "ConversationTurn") \/ ((phase # "CallingLlm") /\ (phase # "WaitingForOps") /\ (phase # "ErrorRecovery")))
terminal_states_match_terminal_outcome == (((phase # "Completed") \/ (terminal_outcome = "Completed") \/ (terminal_outcome = "BudgetExhausted")) /\ ((phase # "Failed") \/ (terminal_outcome = "Failed")) /\ ((phase # "Cancelled") \/ (terminal_outcome = "Cancelled")))
completed_runs_have_seen_a_boundary == ((phase # "Completed") \/ (boundary_count > 0))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []ready_has_no_active_run
THEOREM Spec => []ready_has_no_admitted_content
THEOREM Spec => []non_ready_has_active_run
THEOREM Spec => []waiting_for_ops_implies_pending_tools
THEOREM Spec => []pending_op_ids_only_used_while_waiting
THEOREM Spec => []ready_has_no_boundary_cancel_request
THEOREM Spec => []immediate_primitives_skip_llm_and_recovery
THEOREM Spec => []terminal_states_match_terminal_outcome
THEOREM Spec => []completed_runs_have_seen_a_boundary

=============================================================================
