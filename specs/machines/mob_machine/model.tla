---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MobMachine.

CONSTANTS AgentIdentityValues, AgentRuntimeIdValues, FenceTokenValues, GenerationValues, WorkIdValues

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

VARIABLES phase, model_step_count, active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count

vars == << phase, model_step_count, active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count >>

Init ==
    /\ phase = "Creating"
    /\ model_step_count = 0
    /\ active_identity = None
    /\ active_runtime_id = None
    /\ active_fence_token = None
    /\ current_generation = None
    /\ inflight_work_id = None
    /\ active_member_count = 0
    /\ active_run_count = 0

TerminalStutter ==
    /\ phase = "Destroyed"
    /\ UNCHANGED vars

Start ==
    /\ phase = "Creating" \/ phase = "Stopped"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count >>


SpawnMember(agent_identity, agent_runtime_id, fence_token, generation) ==
    /\ phase = "Creating" \/ phase = "Running" \/ phase = "Stopped"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_identity' = Some(agent_identity)
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ current_generation' = Some(generation)
    /\ active_member_count' = 1
    /\ UNCHANGED << inflight_work_id, active_run_count >>


ObserveRuntimeReady(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count >>


SubmitWork(agent_runtime_id, fence_token, work_id) ==
    /\ phase = "Running"
    /\ (active_runtime_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = Some(work_id)
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count >>


ObserveWorkCompleted(agent_runtime_id, fence_token, work_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count >>


ObserveWorkFailed(agent_runtime_id, fence_token, work_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count >>


ObserveWorkCancelled(agent_runtime_id, fence_token, work_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count >>


RetireMember(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count >>


ObserveRuntimeRetired(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ UNCHANGED << active_identity, current_generation, active_member_count >>


ResetMember(agent_identity, agent_runtime_id, fence_token, generation) ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_identity' = Some(agent_identity)
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ current_generation' = Some(generation)
    /\ inflight_work_id' = None
    /\ UNCHANGED << active_member_count, active_run_count >>


RespawnMember(agent_identity, agent_runtime_id, fence_token, generation) ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_identity' = Some(agent_identity)
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ current_generation' = Some(generation)
    /\ inflight_work_id' = None
    /\ UNCHANGED << active_member_count, active_run_count >>


MarkCompleted ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (inflight_work_id = None)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count >>


DestroyMob ==
    /\ phase = "Creating" \/ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count >>


ObserveRuntimeDestroyed(agent_runtime_id, fence_token) ==
    /\ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed" \/ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ active_member_count' = 0
    /\ active_run_count' = 0
    /\ UNCHANGED << active_identity, current_generation, inflight_work_id >>


Next ==
    \/ Start
    \/ \E agent_identity \in AgentIdentityValues : \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : SpawnMember(agent_identity, agent_runtime_id, fence_token, generation)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : ObserveRuntimeReady(agent_runtime_id, fence_token)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E work_id \in WorkIdValues : SubmitWork(agent_runtime_id, fence_token, work_id)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E work_id \in WorkIdValues : ObserveWorkCompleted(agent_runtime_id, fence_token, work_id)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E work_id \in WorkIdValues : ObserveWorkFailed(agent_runtime_id, fence_token, work_id)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E work_id \in WorkIdValues : ObserveWorkCancelled(agent_runtime_id, fence_token, work_id)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : RetireMember(agent_runtime_id, fence_token)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : ObserveRuntimeRetired(agent_runtime_id, fence_token)
    \/ \E agent_identity \in AgentIdentityValues : \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : ResetMember(agent_identity, agent_runtime_id, fence_token, generation)
    \/ \E agent_identity \in AgentIdentityValues : \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : RespawnMember(agent_identity, agent_runtime_id, fence_token, generation)
    \/ MarkCompleted
    \/ DestroyMob
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : ObserveRuntimeDestroyed(agent_runtime_id, fence_token)
    \/ TerminalStutter

active_work_requires_runtime == ((inflight_work_id = None) \/ (active_runtime_id # None))
destroyed_has_no_active_runtime == ((phase # "Destroyed") \/ (active_runtime_id = None))
identity_and_runtime_move_together == ((active_identity = None) \/ (active_runtime_id # None))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []active_work_requires_runtime
THEOREM Spec => []destroyed_has_no_active_runtime
THEOREM Spec => []identity_and_runtime_move_together

=============================================================================
