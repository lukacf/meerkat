---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for TurnExecutionMachine.

CONSTANTS NatValues, RunIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]
MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, active_run, primitive_kind, tool_calls_pending, boundary_count, terminal_outcome

vars == << phase, model_step_count, active_run, primitive_kind, tool_calls_pending, boundary_count, terminal_outcome >>

Init ==
    /\ phase = "Ready"
    /\ model_step_count = 0
    /\ active_run = None
    /\ primitive_kind = "None"
    /\ tool_calls_pending = 0
    /\ boundary_count = 0
    /\ terminal_outcome = "None"

TerminalStutter ==
    /\ phase = "Completed" \/ phase = "Failed" \/ phase = "Cancelled"
    /\ UNCHANGED vars

StartConversationRun(run_id) ==
    /\ phase = "Ready"
    /\ phase' = "ApplyingPrimitive"
    /\ model_step_count' = model_step_count + 1
    /\ active_run' = Some(run_id)
    /\ primitive_kind' = "ConversationTurn"
    /\ tool_calls_pending' = 0
    /\ boundary_count' = 0
    /\ terminal_outcome' = "None"


StartImmediateAppend(run_id) ==
    /\ phase = "Ready"
    /\ phase' = "ApplyingPrimitive"
    /\ model_step_count' = model_step_count + 1
    /\ active_run' = Some(run_id)
    /\ primitive_kind' = "ImmediateAppend"
    /\ tool_calls_pending' = 0
    /\ boundary_count' = 0
    /\ terminal_outcome' = "None"


StartImmediateContext(run_id) ==
    /\ phase = "Ready"
    /\ phase' = "ApplyingPrimitive"
    /\ model_step_count' = model_step_count + 1
    /\ active_run' = Some(run_id)
    /\ primitive_kind' = "ImmediateContextAppend"
    /\ tool_calls_pending' = 0
    /\ boundary_count' = 0
    /\ terminal_outcome' = "None"


PrimitiveAppliedConversationTurn(run_id) ==
    /\ phase = "ApplyingPrimitive"
    /\ (active_run = Some(run_id))
    /\ (primitive_kind = "ConversationTurn")
    /\ phase' = "CallingLlm"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, boundary_count, terminal_outcome >>


PrimitiveAppliedImmediateAppend(run_id) ==
    /\ phase = "ApplyingPrimitive"
    /\ (active_run = Some(run_id))
    /\ (primitive_kind = "ImmediateAppend")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ boundary_count' = (boundary_count) + 1
    /\ terminal_outcome' = "Completed"
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending >>


PrimitiveAppliedImmediateContext(run_id) ==
    /\ phase = "ApplyingPrimitive"
    /\ (active_run = Some(run_id))
    /\ (primitive_kind = "ImmediateContextAppend")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ boundary_count' = (boundary_count) + 1
    /\ terminal_outcome' = "Completed"
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending >>


LlmReturnedToolCalls(run_id, tool_count) ==
    /\ phase = "CallingLlm"
    /\ (active_run = Some(run_id))
    /\ (tool_count > 0)
    /\ phase' = "WaitingForOps"
    /\ model_step_count' = model_step_count + 1
    /\ tool_calls_pending' = tool_count
    /\ UNCHANGED << active_run, primitive_kind, boundary_count, terminal_outcome >>


ToolCallsResolved(run_id) ==
    /\ phase = "WaitingForOps"
    /\ (active_run = Some(run_id))
    /\ (tool_calls_pending > 0)
    /\ phase' = "DrainingBoundary"
    /\ model_step_count' = model_step_count + 1
    /\ tool_calls_pending' = 0
    /\ boundary_count' = (boundary_count) + 1
    /\ UNCHANGED << active_run, primitive_kind, terminal_outcome >>


LlmReturnedTerminal(run_id) ==
    /\ phase = "CallingLlm"
    /\ (active_run = Some(run_id))
    /\ phase' = "DrainingBoundary"
    /\ model_step_count' = model_step_count + 1
    /\ boundary_count' = (boundary_count) + 1
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, terminal_outcome >>


BoundaryContinue(run_id) ==
    /\ phase = "DrainingBoundary"
    /\ (active_run = Some(run_id))
    /\ (primitive_kind = "ConversationTurn")
    /\ phase' = "CallingLlm"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, boundary_count, terminal_outcome >>


BoundaryComplete(run_id) ==
    /\ phase = "DrainingBoundary"
    /\ (active_run = Some(run_id))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Completed"
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, boundary_count >>


RecoverableFailureFromCallingLlm(run_id) ==
    /\ phase = "CallingLlm"
    /\ (active_run = Some(run_id))
    /\ phase' = "ErrorRecovery"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, boundary_count, terminal_outcome >>


RecoverableFailureFromWaitingForOps(run_id) ==
    /\ phase = "WaitingForOps"
    /\ (active_run = Some(run_id))
    /\ phase' = "ErrorRecovery"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, boundary_count, terminal_outcome >>


RecoverableFailureFromDrainingBoundary(run_id) ==
    /\ phase = "DrainingBoundary"
    /\ (active_run = Some(run_id))
    /\ phase' = "ErrorRecovery"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, boundary_count, terminal_outcome >>


RetryRequested(run_id) ==
    /\ phase = "ErrorRecovery"
    /\ (active_run = Some(run_id))
    /\ phase' = "CallingLlm"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, boundary_count, terminal_outcome >>


FatalFailureFromApplyingPrimitive(run_id) ==
    /\ phase = "ApplyingPrimitive"
    /\ (active_run = Some(run_id))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Failed"
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, boundary_count >>


FatalFailureFromCallingLlm(run_id) ==
    /\ phase = "CallingLlm"
    /\ (active_run = Some(run_id))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Failed"
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, boundary_count >>


FatalFailureFromWaitingForOps(run_id) ==
    /\ phase = "WaitingForOps"
    /\ (active_run = Some(run_id))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Failed"
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, boundary_count >>


FatalFailureFromDrainingBoundary(run_id) ==
    /\ phase = "DrainingBoundary"
    /\ (active_run = Some(run_id))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Failed"
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, boundary_count >>


FatalFailureFromErrorRecovery(run_id) ==
    /\ phase = "ErrorRecovery"
    /\ (active_run = Some(run_id))
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Failed"
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, boundary_count >>


CancelRequestedFromApplyingPrimitive(run_id) ==
    /\ phase = "ApplyingPrimitive"
    /\ (active_run = Some(run_id))
    /\ phase' = "Cancelling"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, boundary_count, terminal_outcome >>


CancelRequestedFromCallingLlm(run_id) ==
    /\ phase = "CallingLlm"
    /\ (active_run = Some(run_id))
    /\ phase' = "Cancelling"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, boundary_count, terminal_outcome >>


CancelRequestedFromWaitingForOps(run_id) ==
    /\ phase = "WaitingForOps"
    /\ (active_run = Some(run_id))
    /\ phase' = "Cancelling"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, boundary_count, terminal_outcome >>


CancelRequestedFromDrainingBoundary(run_id) ==
    /\ phase = "DrainingBoundary"
    /\ (active_run = Some(run_id))
    /\ phase' = "Cancelling"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, boundary_count, terminal_outcome >>


CancelRequestedFromErrorRecovery(run_id) ==
    /\ phase = "ErrorRecovery"
    /\ (active_run = Some(run_id))
    /\ phase' = "Cancelling"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, boundary_count, terminal_outcome >>


CancellationObserved(run_id) ==
    /\ phase = "Cancelling"
    /\ (active_run = Some(run_id))
    /\ phase' = "Cancelled"
    /\ model_step_count' = model_step_count + 1
    /\ terminal_outcome' = "Cancelled"
    /\ UNCHANGED << active_run, primitive_kind, tool_calls_pending, boundary_count >>


AcknowledgeTerminalFromCompleted(run_id) ==
    /\ phase = "Completed"
    /\ (active_run = Some(run_id))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ active_run' = None
    /\ primitive_kind' = "None"
    /\ tool_calls_pending' = 0
    /\ boundary_count' = 0
    /\ terminal_outcome' = "None"


AcknowledgeTerminalFromFailed(run_id) ==
    /\ phase = "Failed"
    /\ (active_run = Some(run_id))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ active_run' = None
    /\ primitive_kind' = "None"
    /\ tool_calls_pending' = 0
    /\ boundary_count' = 0
    /\ terminal_outcome' = "None"


AcknowledgeTerminalFromCancelled(run_id) ==
    /\ phase = "Cancelled"
    /\ (active_run = Some(run_id))
    /\ phase' = "Ready"
    /\ model_step_count' = model_step_count + 1
    /\ active_run' = None
    /\ primitive_kind' = "None"
    /\ tool_calls_pending' = 0
    /\ boundary_count' = 0
    /\ terminal_outcome' = "None"


Next ==
    \/ \E run_id \in RunIdValues : StartConversationRun(run_id)
    \/ \E run_id \in RunIdValues : StartImmediateAppend(run_id)
    \/ \E run_id \in RunIdValues : StartImmediateContext(run_id)
    \/ \E run_id \in RunIdValues : PrimitiveAppliedConversationTurn(run_id)
    \/ \E run_id \in RunIdValues : PrimitiveAppliedImmediateAppend(run_id)
    \/ \E run_id \in RunIdValues : PrimitiveAppliedImmediateContext(run_id)
    \/ \E run_id \in RunIdValues : \E tool_count \in 0..2 : LlmReturnedToolCalls(run_id, tool_count)
    \/ \E run_id \in RunIdValues : ToolCallsResolved(run_id)
    \/ \E run_id \in RunIdValues : LlmReturnedTerminal(run_id)
    \/ \E run_id \in RunIdValues : BoundaryContinue(run_id)
    \/ \E run_id \in RunIdValues : BoundaryComplete(run_id)
    \/ \E run_id \in RunIdValues : RecoverableFailureFromCallingLlm(run_id)
    \/ \E run_id \in RunIdValues : RecoverableFailureFromWaitingForOps(run_id)
    \/ \E run_id \in RunIdValues : RecoverableFailureFromDrainingBoundary(run_id)
    \/ \E run_id \in RunIdValues : RetryRequested(run_id)
    \/ \E run_id \in RunIdValues : FatalFailureFromApplyingPrimitive(run_id)
    \/ \E run_id \in RunIdValues : FatalFailureFromCallingLlm(run_id)
    \/ \E run_id \in RunIdValues : FatalFailureFromWaitingForOps(run_id)
    \/ \E run_id \in RunIdValues : FatalFailureFromDrainingBoundary(run_id)
    \/ \E run_id \in RunIdValues : FatalFailureFromErrorRecovery(run_id)
    \/ \E run_id \in RunIdValues : CancelRequestedFromApplyingPrimitive(run_id)
    \/ \E run_id \in RunIdValues : CancelRequestedFromCallingLlm(run_id)
    \/ \E run_id \in RunIdValues : CancelRequestedFromWaitingForOps(run_id)
    \/ \E run_id \in RunIdValues : CancelRequestedFromDrainingBoundary(run_id)
    \/ \E run_id \in RunIdValues : CancelRequestedFromErrorRecovery(run_id)
    \/ \E run_id \in RunIdValues : CancellationObserved(run_id)
    \/ \E run_id \in RunIdValues : AcknowledgeTerminalFromCompleted(run_id)
    \/ \E run_id \in RunIdValues : AcknowledgeTerminalFromFailed(run_id)
    \/ \E run_id \in RunIdValues : AcknowledgeTerminalFromCancelled(run_id)
    \/ TerminalStutter

ready_has_no_active_run == ((phase # "Ready") \/ (active_run = None))
non_ready_has_active_run == ((phase = "Ready") \/ (active_run # None))
waiting_for_ops_implies_pending_tools == ((phase # "WaitingForOps") \/ (tool_calls_pending > 0))
immediate_primitives_skip_llm_and_recovery == ((primitive_kind = "ConversationTurn") \/ ((phase # "CallingLlm") /\ (phase # "WaitingForOps") /\ (phase # "ErrorRecovery")))
terminal_states_match_terminal_outcome == (((phase # "Completed") \/ (terminal_outcome = "Completed")) /\ ((phase # "Failed") \/ (terminal_outcome = "Failed")) /\ ((phase # "Cancelled") \/ (terminal_outcome = "Cancelled")))
completed_runs_have_seen_a_boundary == ((phase # "Completed") \/ (boundary_count > 0))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []ready_has_no_active_run
THEOREM Spec => []non_ready_has_active_run
THEOREM Spec => []waiting_for_ops_implies_pending_tools
THEOREM Spec => []immediate_primitives_skip_llm_and_recovery
THEOREM Spec => []terminal_states_match_terminal_outcome
THEOREM Spec => []completed_runs_have_seen_a_boundary

=============================================================================
