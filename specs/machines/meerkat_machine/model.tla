---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MeerkatMachine.

CONSTANTS AgentRuntimeIdValues, FenceTokenValues, GenerationValues, NatValues, SessionIdValues, WorkIdValues

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

VARIABLES phase, model_step_count, session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, committed_visibility_revision

vars == << phase, model_step_count, session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, committed_visibility_revision >>

Init ==
    /\ phase = "Initializing"
    /\ model_step_count = 0
    /\ session_id = None
    /\ active_runtime_id = None
    /\ active_fence_token = None
    /\ active_generation = None
    /\ active_work_id = None
    /\ wake_pending = FALSE
    /\ process_pending = FALSE
    /\ committed_visibility_revision = 0

TerminalStutter ==
    /\ phase = "Destroyed"
    /\ UNCHANGED vars

Initialize ==
    /\ phase = "Initializing"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, committed_visibility_revision >>


RegisterSession(arg_session_id) ==
    /\ phase = "Idle" \/ phase = "Stopped" \/ phase = "Retired"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, committed_visibility_revision >>


PrepareBindings(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Idle" \/ phase = "Stopped" \/ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ active_generation' = Some(generation)
    /\ UNCHANGED << session_id, active_work_id, wake_pending, process_pending, committed_visibility_revision >>


BeginRunFromIdle(agent_runtime_id, fence_token, work_id) ==
    /\ phase = "Attached"
    /\ (active_runtime_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_work_id' = Some(work_id)
    /\ wake_pending' = TRUE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, process_pending, committed_visibility_revision >>


AdmissionAcceptedIdleSteer(agent_runtime_id, fence_token, work_id) ==
    /\ phase = "Attached"
    /\ (active_runtime_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_work_id' = Some(work_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, wake_pending, process_pending, committed_visibility_revision >>


InterruptCurrentRun ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, committed_visibility_revision >>


CancelAfterBoundary ==
    /\ phase = "Running"
    /\ (active_work_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, committed_visibility_revision >>


BoundaryApplied(revision) ==
    /\ phase = "Running" \/ phase = "Attached"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ committed_visibility_revision' = revision
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending >>


PublishCommittedVisibleSet(revision) ==
    /\ phase = "Attached" \/ phase = "Running"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ committed_visibility_revision' = revision
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending >>


RunCompleted(work_id) ==
    /\ phase = "Running"
    /\ (active_work_id = Some(work_id))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_work_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, wake_pending, process_pending, committed_visibility_revision >>


RunFailed(work_id) ==
    /\ phase = "Running"
    /\ (active_work_id = Some(work_id))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_work_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, wake_pending, process_pending, committed_visibility_revision >>


RunCancelled(work_id) ==
    /\ phase = "Running"
    /\ (active_work_id = Some(work_id))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_work_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, wake_pending, process_pending, committed_visibility_revision >>


RecoverRuntime ==
    /\ phase = "Idle" \/ phase = "Stopped" \/ phase = "Retired"
    /\ phase' = "Recovering"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, active_work_id, wake_pending, process_pending, committed_visibility_revision >>


RetireRequestedFromIdle ==
    /\ phase = "Attached" \/ phase = "Running"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ active_work_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, wake_pending, process_pending, committed_visibility_revision >>


ResetRuntime ==
    /\ phase = "Attached" \/ phase = "Retired" \/ phase = "Stopped" \/ phase = "Recovering"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ active_generation' = None
    /\ active_work_id' = None
    /\ UNCHANGED << session_id, wake_pending, process_pending, committed_visibility_revision >>


StopRuntimeExecutor ==
    /\ phase = "Attached" \/ phase = "Retired" \/ phase = "Recovering"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_work_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, wake_pending, process_pending, committed_visibility_revision >>


DestroyRuntime ==
    /\ phase = "Attached" \/ phase = "Running" \/ phase = "Recovering" \/ phase = "Retired" \/ phase = "Stopped"
    /\ (active_runtime_id # None)
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ active_work_id' = None
    /\ wake_pending' = FALSE
    /\ process_pending' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, committed_visibility_revision >>


Next ==
    \/ Initialize
    \/ \E arg_session_id \in SessionIdValues : RegisterSession(arg_session_id)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : PrepareBindings(agent_runtime_id, fence_token, generation)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E work_id \in WorkIdValues : BeginRunFromIdle(agent_runtime_id, fence_token, work_id)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E work_id \in WorkIdValues : AdmissionAcceptedIdleSteer(agent_runtime_id, fence_token, work_id)
    \/ InterruptCurrentRun
    \/ CancelAfterBoundary
    \/ \E revision \in 0..2 : BoundaryApplied(revision)
    \/ \E revision \in 0..2 : PublishCommittedVisibleSet(revision)
    \/ \E work_id \in WorkIdValues : RunCompleted(work_id)
    \/ \E work_id \in WorkIdValues : RunFailed(work_id)
    \/ \E work_id \in WorkIdValues : RunCancelled(work_id)
    \/ RecoverRuntime
    \/ RetireRequestedFromIdle
    \/ ResetRuntime
    \/ StopRuntimeExecutor
    \/ DestroyRuntime
    \/ TerminalStutter

running_has_active_work == ((phase # "Running") \/ (active_work_id # None))
bound_runtime_has_fence == ((active_runtime_id = None) \/ (active_fence_token # None))
destroyed_has_no_active_work == ((phase # "Destroyed") \/ (active_work_id = None))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []running_has_active_work
THEOREM Spec => []bound_runtime_has_fence
THEOREM Spec => []destroyed_has_no_active_work

=============================================================================
