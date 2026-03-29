---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for LoopIterationMachine.

CONSTANTS FrameIdValues, LoopInstanceIdValues, NatValues

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

VARIABLES phase, model_step_count, loop_instance_id, current_iteration, max_iterations, active_body_frame_id, loop_failure_kind

vars == << phase, model_step_count, loop_instance_id, current_iteration, max_iterations, active_body_frame_id, loop_failure_kind >>

Init ==
    /\ phase = "Absent"
    /\ model_step_count = 0
    /\ loop_instance_id = ""
    /\ current_iteration = 0
    /\ max_iterations = 0
    /\ active_body_frame_id = None
    /\ loop_failure_kind = None

TerminalStutter ==
    /\ phase = "Completed" \/ phase = "Exhausted" \/ phase = "Failed" \/ phase = "Canceled"
    /\ UNCHANGED vars

StartLoop(arg_loop_instance_id, arg_max_iterations) ==
    /\ phase = "Absent"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ loop_instance_id' = arg_loop_instance_id
    /\ current_iteration' = 0
    /\ max_iterations' = arg_max_iterations
    /\ UNCHANGED << active_body_frame_id, loop_failure_kind >>


BodyFrameStarted(frame_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_body_frame_id' = Some(frame_id)
    /\ UNCHANGED << loop_instance_id, current_iteration, max_iterations, loop_failure_kind >>


BodyFrameCompleted ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_iteration' = (current_iteration) + 1
    /\ active_body_frame_id' = None
    /\ UNCHANGED << loop_instance_id, max_iterations, loop_failure_kind >>


UntilConditionMet ==
    /\ phase = "Running"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << loop_instance_id, current_iteration, max_iterations, active_body_frame_id, loop_failure_kind >>


UntilConditionFailed ==
    /\ phase = "Running"
    /\ (current_iteration < max_iterations)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << loop_instance_id, current_iteration, max_iterations, active_body_frame_id, loop_failure_kind >>


ExhaustedIterations ==
    /\ phase = "Running"
    /\ (current_iteration >= max_iterations)
    /\ phase' = "Exhausted"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << loop_instance_id, current_iteration, max_iterations, active_body_frame_id, loop_failure_kind >>


BodyFrameFailed ==
    /\ phase = "Running"
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ active_body_frame_id' = None
    /\ UNCHANGED << loop_instance_id, current_iteration, max_iterations, loop_failure_kind >>


BodyFrameCanceled ==
    /\ phase = "Running"
    /\ phase' = "Canceled"
    /\ model_step_count' = model_step_count + 1
    /\ active_body_frame_id' = None
    /\ UNCHANGED << loop_instance_id, current_iteration, max_iterations, loop_failure_kind >>


CancelLoop ==
    /\ phase = "Running"
    /\ phase' = "Canceled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << loop_instance_id, current_iteration, max_iterations, active_body_frame_id, loop_failure_kind >>


Next ==
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_max_iterations \in 0..2 : StartLoop(arg_loop_instance_id, arg_max_iterations)
    \/ \E frame_id \in FrameIdValues : BodyFrameStarted(frame_id)
    \/ BodyFrameCompleted
    \/ UntilConditionMet
    \/ UntilConditionFailed
    \/ ExhaustedIterations
    \/ BodyFrameFailed
    \/ BodyFrameCanceled
    \/ CancelLoop
    \/ TerminalStutter


CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars


=============================================================================
