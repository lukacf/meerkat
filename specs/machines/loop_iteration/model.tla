---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for LoopIterationMachine.

CONSTANTS FlowNodeIdValues, FrameIdValues, LoopIdValues, LoopInstanceIdValues, NatValues

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

VARIABLES phase, model_step_count, loop_instance_id, parent_frame_id, parent_node_id, loop_id, depth, stage, current_iteration, last_completed_iteration, max_iterations, active_body_frame_id

vars == << phase, model_step_count, loop_instance_id, parent_frame_id, parent_node_id, loop_id, depth, stage, current_iteration, last_completed_iteration, max_iterations, active_body_frame_id >>

Init ==
    /\ phase = "Absent"
    /\ model_step_count = 0
    /\ loop_instance_id = ""
    /\ parent_frame_id = ""
    /\ parent_node_id = ""
    /\ loop_id = ""
    /\ depth = 0
    /\ stage = "AwaitingBodyFrame"
    /\ current_iteration = 0
    /\ last_completed_iteration = 0
    /\ max_iterations = 0
    /\ active_body_frame_id = None

TerminalStutter ==
    /\ phase = "Completed" \/ phase = "Exhausted" \/ phase = "Failed" \/ phase = "Canceled"
    /\ UNCHANGED vars

StartLoop(arg_loop_instance_id, arg_max_iterations, arg_parent_frame_id, arg_parent_node_id, arg_loop_id, arg_depth) ==
    /\ phase = "Absent"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ loop_instance_id' = arg_loop_instance_id
    /\ parent_frame_id' = arg_parent_frame_id
    /\ parent_node_id' = arg_parent_node_id
    /\ loop_id' = arg_loop_id
    /\ depth' = arg_depth
    /\ stage' = "AwaitingBodyFrame"
    /\ current_iteration' = 0
    /\ last_completed_iteration' = 0
    /\ max_iterations' = arg_max_iterations
    /\ active_body_frame_id' = None


BodyFrameStarted(arg_loop_instance_id, frame_id, iteration) ==
    /\ phase = "Running"
    /\ (loop_instance_id = arg_loop_instance_id)
    /\ (stage = "AwaitingBodyFrame")
    /\ (current_iteration = iteration)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ stage' = "BodyFrameActive"
    /\ active_body_frame_id' = Some(frame_id)
    /\ UNCHANGED << loop_instance_id, parent_frame_id, parent_node_id, loop_id, depth, current_iteration, last_completed_iteration, max_iterations >>


BodyFrameCompleted(arg_loop_instance_id, iteration) ==
    /\ phase = "Running"
    /\ (loop_instance_id = arg_loop_instance_id)
    /\ (stage = "BodyFrameActive")
    /\ (current_iteration = iteration)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ stage' = "AwaitingUntil"
    /\ current_iteration' = (current_iteration) + 1
    /\ last_completed_iteration' = iteration
    /\ active_body_frame_id' = None
    /\ UNCHANGED << loop_instance_id, parent_frame_id, parent_node_id, loop_id, depth, max_iterations >>


UntilConditionMet(arg_loop_instance_id, iteration) ==
    /\ phase = "Running"
    /\ (loop_instance_id = arg_loop_instance_id)
    /\ (stage = "AwaitingUntil")
    /\ (last_completed_iteration = iteration)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << loop_instance_id, parent_frame_id, parent_node_id, loop_id, depth, stage, current_iteration, last_completed_iteration, max_iterations, active_body_frame_id >>


UntilConditionFailed(arg_loop_instance_id, iteration) ==
    /\ phase = "Running"
    /\ (loop_instance_id = arg_loop_instance_id)
    /\ (stage = "AwaitingUntil")
    /\ (last_completed_iteration = iteration)
    /\ (current_iteration < max_iterations)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ stage' = "AwaitingBodyFrame"
    /\ UNCHANGED << loop_instance_id, parent_frame_id, parent_node_id, loop_id, depth, current_iteration, last_completed_iteration, max_iterations, active_body_frame_id >>


ExhaustedIterations(arg_loop_instance_id, iteration) ==
    /\ phase = "Running"
    /\ (loop_instance_id = arg_loop_instance_id)
    /\ (stage = "AwaitingUntil")
    /\ (last_completed_iteration = iteration)
    /\ (current_iteration >= max_iterations)
    /\ phase' = "Exhausted"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << loop_instance_id, parent_frame_id, parent_node_id, loop_id, depth, stage, current_iteration, last_completed_iteration, max_iterations, active_body_frame_id >>


BodyFrameFailed(arg_loop_instance_id, iteration) ==
    /\ phase = "Running"
    /\ (loop_instance_id = arg_loop_instance_id)
    /\ (stage = "BodyFrameActive")
    /\ (current_iteration = iteration)
    /\ phase' = "Failed"
    /\ model_step_count' = model_step_count + 1
    /\ active_body_frame_id' = None
    /\ UNCHANGED << loop_instance_id, parent_frame_id, parent_node_id, loop_id, depth, stage, current_iteration, last_completed_iteration, max_iterations >>


BodyFrameCanceled(arg_loop_instance_id, iteration) ==
    /\ phase = "Running"
    /\ (loop_instance_id = arg_loop_instance_id)
    /\ (stage = "BodyFrameActive")
    /\ (current_iteration = iteration)
    /\ phase' = "Canceled"
    /\ model_step_count' = model_step_count + 1
    /\ active_body_frame_id' = None
    /\ UNCHANGED << loop_instance_id, parent_frame_id, parent_node_id, loop_id, depth, stage, current_iteration, last_completed_iteration, max_iterations >>


CancelLoop(arg_loop_instance_id) ==
    /\ phase = "Running"
    /\ (loop_instance_id = arg_loop_instance_id)
    /\ phase' = "Canceled"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << loop_instance_id, parent_frame_id, parent_node_id, loop_id, depth, stage, current_iteration, last_completed_iteration, max_iterations, active_body_frame_id >>


Next ==
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E arg_max_iterations \in 0..2 : \E arg_parent_frame_id \in FrameIdValues : \E arg_parent_node_id \in FlowNodeIdValues : \E arg_loop_id \in LoopIdValues : \E arg_depth \in 0..2 : StartLoop(arg_loop_instance_id, arg_max_iterations, arg_parent_frame_id, arg_parent_node_id, arg_loop_id, arg_depth)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E frame_id \in FrameIdValues : \E iteration \in 0..2 : BodyFrameStarted(arg_loop_instance_id, frame_id, iteration)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E iteration \in 0..2 : BodyFrameCompleted(arg_loop_instance_id, iteration)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E iteration \in 0..2 : UntilConditionMet(arg_loop_instance_id, iteration)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E iteration \in 0..2 : UntilConditionFailed(arg_loop_instance_id, iteration)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E iteration \in 0..2 : ExhaustedIterations(arg_loop_instance_id, iteration)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E iteration \in 0..2 : BodyFrameFailed(arg_loop_instance_id, iteration)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : \E iteration \in 0..2 : BodyFrameCanceled(arg_loop_instance_id, iteration)
    \/ \E arg_loop_instance_id \in LoopInstanceIdValues : CancelLoop(arg_loop_instance_id)
    \/ TerminalStutter


CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars


=============================================================================
