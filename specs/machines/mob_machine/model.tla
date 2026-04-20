---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MobMachine.

CONSTANTS AgentIdentityValues, AgentRuntimeIdValues, BooleanValues, FenceTokenValues, GenerationValues, MobMemberStateValues, MobTaskValues, NatValues, SessionIdValues, SetOfAgentRuntimeIdValues, SetOfTaskIdValues, SetOfWiringEdgeValues, TaskIdValues, TaskStatusValues, WiringEdgeValues, WorkIdValues, WorkOriginValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionSessionIdValues == {None} \cup {Some(x) : x \in SessionIdValues}
MapAgentIdentityAgentRuntimeIdValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in AgentIdentityValues, v \in AgentRuntimeIdValues }
MapAgentIdentitySessionIdValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in AgentIdentityValues, v \in SessionIdValues }
MapAgentRuntimeIdFenceTokenValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in AgentRuntimeIdValues, v \in FenceTokenValues }
MapAgentRuntimeIdMobMemberStateValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in AgentRuntimeIdValues, v \in MobMemberStateValues }
MapTaskIdMobTaskValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in TaskIdValues, v \in MobTaskValues }

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
MapRemove(map, key) == [x \in DOMAIN map \ {key} |-> map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings

vars == << phase, model_step_count, live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>

Init ==
    /\ phase = "Running"
    /\ model_step_count = 0
    /\ live_runtime_ids = {}
    /\ externally_addressable_runtime_ids = {}
    /\ runtime_fence_tokens = [x \in {} |-> None]
    /\ active_run_count = 0
    /\ pending_spawn_count = 0
    /\ coordinator_bound = TRUE
    /\ member_state_markers = [x \in {} |-> None]
    /\ wiring_edges = {}
    /\ identity_to_runtime = [x \in {} |-> None]
    /\ tasks = [x \in {} |-> None]
    /\ in_progress_task_ids = {}
    /\ completed_task_ids = {}
    /\ member_realtime_bindings = [x \in {} |-> None]

TerminalStutter ==
    /\ phase = "Destroyed"
    /\ UNCHANGED vars

SpawnRunningFresh(agent_identity, agent_runtime_id, fence_token, generation, external_addressable, bridge_session_id, replacing) ==
    /\ phase = "Running"
    /\ (coordinator_bound = TRUE)
    /\ ((agent_identity \in DOMAIN member_realtime_bindings) = FALSE)
    /\ (replacing = None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = (live_runtime_ids \cup {agent_runtime_id})
    /\ externally_addressable_runtime_ids' = IF external_addressable THEN (externally_addressable_runtime_ids \cup {agent_runtime_id}) ELSE (externally_addressable_runtime_ids \ {agent_runtime_id})
    /\ runtime_fence_tokens' = MapSet(runtime_fence_tokens, agent_runtime_id, fence_token)
    /\ identity_to_runtime' = MapSet(identity_to_runtime, agent_identity, agent_runtime_id)
    /\ member_realtime_bindings' = MapSet(member_realtime_bindings, agent_identity, bridge_session_id)
    /\ UNCHANGED << active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, tasks, in_progress_task_ids, completed_task_ids >>


SpawnRunningReplacing(agent_identity, agent_runtime_id, fence_token, generation, external_addressable, bridge_session_id, replacing) ==
    /\ phase = "Running"
    /\ (coordinator_bound = TRUE)
    /\ ((agent_identity \in DOMAIN member_realtime_bindings) = TRUE)
    /\ (replacing # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = (live_runtime_ids \cup {agent_runtime_id})
    /\ externally_addressable_runtime_ids' = IF external_addressable THEN (externally_addressable_runtime_ids \cup {agent_runtime_id}) ELSE (externally_addressable_runtime_ids \ {agent_runtime_id})
    /\ runtime_fence_tokens' = MapSet(runtime_fence_tokens, agent_runtime_id, fence_token)
    /\ identity_to_runtime' = MapSet(identity_to_runtime, agent_identity, agent_runtime_id)
    /\ member_realtime_bindings' = MapSet(member_realtime_bindings, agent_identity, bridge_session_id)
    /\ UNCHANGED << active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, tasks, in_progress_task_ids, completed_task_ids >>


ObserveRuntimeReady(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


SubmitWorkRunningExternal(agent_runtime_id, fence_token, work_id, origin) ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ (origin = "External")
    /\ (agent_runtime_id \in externally_addressable_runtime_ids)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


SubmitWorkRunningInternal(agent_runtime_id, fence_token, work_id, origin) ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ (origin = "Internal")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


RetireMember(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ member_state_markers' = MapSet(member_state_markers, agent_runtime_id, "Retiring")
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


ObserveRuntimeRetired(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = (live_runtime_ids \ {agent_runtime_id})
    /\ externally_addressable_runtime_ids' = (externally_addressable_runtime_ids \ {agent_runtime_id})
    /\ runtime_fence_tokens' = MapRemove(runtime_fence_tokens, agent_runtime_id)
    /\ active_run_count' = 0
    /\ member_state_markers' = MapRemove(member_state_markers, agent_runtime_id)
    /\ UNCHANGED << pending_spawn_count, coordinator_bound, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


ResetMember(agent_identity, agent_runtime_id, fence_token, generation, external_addressable) ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = (live_runtime_ids \cup {agent_runtime_id})
    /\ externally_addressable_runtime_ids' = IF external_addressable THEN (externally_addressable_runtime_ids \cup {agent_runtime_id}) ELSE (externally_addressable_runtime_ids \ {agent_runtime_id})
    /\ runtime_fence_tokens' = MapSet(runtime_fence_tokens, agent_runtime_id, fence_token)
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ identity_to_runtime' = MapSet(identity_to_runtime, agent_identity, agent_runtime_id)
    /\ UNCHANGED << coordinator_bound, member_state_markers, wiring_edges, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


RespawnMember(agent_identity, agent_runtime_id, fence_token, generation, external_addressable) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = (live_runtime_ids \cup {agent_runtime_id})
    /\ externally_addressable_runtime_ids' = IF external_addressable THEN (externally_addressable_runtime_ids \cup {agent_runtime_id}) ELSE (externally_addressable_runtime_ids \ {agent_runtime_id})
    /\ runtime_fence_tokens' = MapSet(runtime_fence_tokens, agent_runtime_id, fence_token)
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ identity_to_runtime' = MapSet(identity_to_runtime, agent_identity, agent_runtime_id)
    /\ UNCHANGED << coordinator_bound, member_state_markers, wiring_edges, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


MarkCompleted ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (active_run_count = 0)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


DestroyMob ==
    /\ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = {}
    /\ runtime_fence_tokens' = [x \in {} |-> None]
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ coordinator_bound' = FALSE
    /\ member_state_markers' = [x \in {} |-> None]
    /\ UNCHANGED << externally_addressable_runtime_ids, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


ObserveRuntimeDestroyed(agent_runtime_id, fence_token) ==
    /\ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed" \/ phase = "Destroyed"
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = {}
    /\ runtime_fence_tokens' = [x \in {} |-> None]
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ coordinator_bound' = FALSE
    /\ member_state_markers' = [x \in {} |-> None]
    /\ UNCHANGED << externally_addressable_runtime_ids, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


RecordOperatorActionProvenanceRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


RecordOperatorActionProvenanceStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


RecordOperatorActionProvenanceCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


RecordOperatorActionProvenanceDestroyed ==
    /\ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


SetSpawnPolicyRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


SetSpawnPolicyStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


SetSpawnPolicyCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


SetSpawnPolicyDestroyed ==
    /\ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


StopRunning ==
    /\ phase = "Running"
    /\ (active_run_count = 0)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


ResumeStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


CompleteRunning ==
    /\ phase = "Running"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


ResetToRunning ==
    /\ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


WireRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


TaskCreateRunning(task_id, task_payload) ==
    /\ phase = "Running"
    /\ ((task_id \in DOMAIN tasks) = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ tasks' = MapSet(tasks, task_id, task_payload)
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


TaskUpdateRunningPending(task_id, new_status) ==
    /\ phase = "Running"
    /\ (new_status = "Pending")
    /\ ((task_id \in DOMAIN tasks) = TRUE)
    /\ ((task_id \in completed_task_ids) = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ in_progress_task_ids' = (in_progress_task_ids \ {task_id})
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, completed_task_ids, member_realtime_bindings >>


TaskUpdateRunningInProgress(task_id, new_status) ==
    /\ phase = "Running"
    /\ (new_status = "InProgress")
    /\ ((task_id \in DOMAIN tasks) = TRUE)
    /\ ((task_id \in completed_task_ids) = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ in_progress_task_ids' = (in_progress_task_ids \cup {task_id})
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, completed_task_ids, member_realtime_bindings >>


TaskUpdateRunningCompleted(task_id, new_status) ==
    /\ phase = "Running"
    /\ (new_status = "Completed")
    /\ ((task_id \in DOMAIN tasks) = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ in_progress_task_ids' = (in_progress_task_ids \ {task_id})
    /\ completed_task_ids' = (completed_task_ids \cup {task_id})
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, member_realtime_bindings >>


TaskUpdateRunningCancelled(task_id, new_status) ==
    /\ phase = "Running"
    /\ (new_status = "Cancelled")
    /\ ((task_id \in DOMAIN tasks) = TRUE)
    /\ ((task_id \in completed_task_ids) = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ in_progress_task_ids' = (in_progress_task_ids \ {task_id})
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, completed_task_ids, member_realtime_bindings >>


ForceCancelRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


SubscribeAgentEventsRunning ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


SubscribeAgentEventsStopped ==
    /\ phase = "Stopped"
    /\ (live_runtime_ids # {})
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


SubscribeAgentEventsCompleted ==
    /\ phase = "Completed"
    /\ (live_runtime_ids # {})
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


SubscribeAgentEventsDestroyed ==
    /\ phase = "Destroyed"
    /\ (live_runtime_ids # {})
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


SubscribeAllAgentEventsRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


SubscribeAllAgentEventsStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


SubscribeAllAgentEventsCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


SubscribeAllAgentEventsDestroyed ==
    /\ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


SubscribeMobEventsRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


SubscribeMobEventsStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


SubscribeMobEventsCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


SubscribeMobEventsDestroyed ==
    /\ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


ShutdownRunning ==
    /\ phase = "Running"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


ShutdownStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


ShutdownCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


CancelFlowRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


InitializeOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


BindCoordinatorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


UnbindCoordinatorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


StageSpawnRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_spawn_count' = (pending_spawn_count) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


StopOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


StopOrchestratorStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


StopOrchestratorCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


ResumeOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


ResumeOrchestratorStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


ResumeOrchestratorCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


DestroyOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


DestroyOrchestratorStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


DestroyOrchestratorCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


ForceCancelMemberRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


MemberPeerExposedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


MemberTerminalizedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


OperationPeerTrustedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


PeerInputAdmittedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


BeginCleanupStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


BeginCleanupCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


FinishCleanupStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


FinishCleanupCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


RunFlowRunning ==
    /\ phase = "Running"
    /\ (coordinator_bound = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


StartFlowRunning ==
    /\ phase = "Running"
    /\ (coordinator_bound = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


CreateRunRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


StartRunRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


UnwireRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


CompleteFlowRunning ==
    /\ phase = "Running" \/ phase = "Completed"
    /\ (active_run_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) - 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


CompleteFlowRunningZero ==
    /\ phase = "Running" \/ phase = "Completed"
    /\ (active_run_count = 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


FinishRunRunning ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (active_run_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) - 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


FinishRunRunningZero ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (active_run_count = 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


RetireRunningReleasing(agent_runtime_id, agent_identity, releasing) ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ ((agent_identity \in DOMAIN member_realtime_bindings) = TRUE)
    /\ (releasing # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ member_state_markers' = MapSet(member_state_markers, agent_runtime_id, "Retiring")
    /\ member_realtime_bindings' = MapRemove(member_realtime_bindings, agent_identity)
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


RetireRunningPreservingBinding(agent_runtime_id, agent_identity, releasing) ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ ((agent_identity \in DOMAIN member_realtime_bindings) = TRUE)
    /\ (releasing = None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ member_state_markers' = MapSet(member_state_markers, agent_runtime_id, "Retiring")
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


RetireRunningNoBinding(agent_runtime_id, agent_identity, releasing) ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ ((agent_identity \in DOMAIN member_realtime_bindings) = FALSE)
    /\ (releasing = None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ member_state_markers' = MapSet(member_state_markers, agent_runtime_id, "Retiring")
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


RetireStoppedReleasing(agent_runtime_id, agent_identity, releasing) ==
    /\ phase = "Stopped"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ ((agent_identity \in DOMAIN member_realtime_bindings) = TRUE)
    /\ (releasing # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ member_state_markers' = MapSet(member_state_markers, agent_runtime_id, "Retiring")
    /\ member_realtime_bindings' = MapRemove(member_realtime_bindings, agent_identity)
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


RetireStoppedPreservingBinding(agent_runtime_id, agent_identity, releasing) ==
    /\ phase = "Stopped"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ ((agent_identity \in DOMAIN member_realtime_bindings) = TRUE)
    /\ (releasing = None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ member_state_markers' = MapSet(member_state_markers, agent_runtime_id, "Retiring")
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


RetireStoppedNoBinding(agent_runtime_id, agent_identity, releasing) ==
    /\ phase = "Stopped"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ ((agent_identity \in DOMAIN member_realtime_bindings) = FALSE)
    /\ (releasing = None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ member_state_markers' = MapSet(member_state_markers, agent_runtime_id, "Retiring")
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


RetireAllRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = {}
    /\ runtime_fence_tokens' = [x \in {} |-> None]
    /\ UNCHANGED << externally_addressable_runtime_ids, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


RetireAllStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = {}
    /\ runtime_fence_tokens' = [x \in {} |-> None]
    /\ UNCHANGED << externally_addressable_runtime_ids, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


CompleteSpawnRunning ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (pending_spawn_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_spawn_count' = (pending_spawn_count) - 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


DestroyFromAny ==
    /\ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = {}
    /\ runtime_fence_tokens' = [x \in {} |-> None]
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << externally_addressable_runtime_ids, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


RespawnRunning(agent_runtime_id) ==
    /\ phase = "Running"
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ (coordinator_bound = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


CancelAllWorkRunning(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_realtime_bindings >>


Next ==
    \/ \E agent_identity \in AgentIdentityValues : \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : \E external_addressable \in BOOLEAN : \E bridge_session_id \in SessionIdValues : \E replacing \in OptionSessionIdValues : SpawnRunningFresh(agent_identity, agent_runtime_id, fence_token, generation, external_addressable, bridge_session_id, replacing)
    \/ \E agent_identity \in AgentIdentityValues : \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : \E external_addressable \in BOOLEAN : \E bridge_session_id \in SessionIdValues : \E replacing \in OptionSessionIdValues : SpawnRunningReplacing(agent_identity, agent_runtime_id, fence_token, generation, external_addressable, bridge_session_id, replacing)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : ObserveRuntimeReady(agent_runtime_id, fence_token)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E work_id \in WorkIdValues : \E origin \in WorkOriginValues : SubmitWorkRunningExternal(agent_runtime_id, fence_token, work_id, origin)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E work_id \in WorkIdValues : \E origin \in WorkOriginValues : SubmitWorkRunningInternal(agent_runtime_id, fence_token, work_id, origin)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : RetireMember(agent_runtime_id, fence_token)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : ObserveRuntimeRetired(agent_runtime_id, fence_token)
    \/ \E agent_identity \in AgentIdentityValues : \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : \E external_addressable \in BOOLEAN : ResetMember(agent_identity, agent_runtime_id, fence_token, generation, external_addressable)
    \/ \E agent_identity \in AgentIdentityValues : \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : \E external_addressable \in BOOLEAN : RespawnMember(agent_identity, agent_runtime_id, fence_token, generation, external_addressable)
    \/ MarkCompleted
    \/ DestroyMob
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : ObserveRuntimeDestroyed(agent_runtime_id, fence_token)
    \/ RecordOperatorActionProvenanceRunning
    \/ RecordOperatorActionProvenanceStopped
    \/ RecordOperatorActionProvenanceCompleted
    \/ RecordOperatorActionProvenanceDestroyed
    \/ SetSpawnPolicyRunning
    \/ SetSpawnPolicyStopped
    \/ SetSpawnPolicyCompleted
    \/ SetSpawnPolicyDestroyed
    \/ StopRunning
    \/ ResumeStopped
    \/ CompleteRunning
    \/ ResetToRunning
    \/ WireRunning
    \/ \E task_id \in TaskIdValues : \E task_payload \in MobTaskValues : TaskCreateRunning(task_id, task_payload)
    \/ \E task_id \in TaskIdValues : \E new_status \in TaskStatusValues : TaskUpdateRunningPending(task_id, new_status)
    \/ \E task_id \in TaskIdValues : \E new_status \in TaskStatusValues : TaskUpdateRunningInProgress(task_id, new_status)
    \/ \E task_id \in TaskIdValues : \E new_status \in TaskStatusValues : TaskUpdateRunningCompleted(task_id, new_status)
    \/ \E task_id \in TaskIdValues : \E new_status \in TaskStatusValues : TaskUpdateRunningCancelled(task_id, new_status)
    \/ ForceCancelRunning
    \/ SubscribeAgentEventsRunning
    \/ SubscribeAgentEventsStopped
    \/ SubscribeAgentEventsCompleted
    \/ SubscribeAgentEventsDestroyed
    \/ SubscribeAllAgentEventsRunning
    \/ SubscribeAllAgentEventsStopped
    \/ SubscribeAllAgentEventsCompleted
    \/ SubscribeAllAgentEventsDestroyed
    \/ SubscribeMobEventsRunning
    \/ SubscribeMobEventsStopped
    \/ SubscribeMobEventsCompleted
    \/ SubscribeMobEventsDestroyed
    \/ ShutdownRunning
    \/ ShutdownStopped
    \/ ShutdownCompleted
    \/ CancelFlowRunning
    \/ InitializeOrchestratorRunning
    \/ BindCoordinatorRunning
    \/ UnbindCoordinatorRunning
    \/ StageSpawnRunning
    \/ StopOrchestratorRunning
    \/ StopOrchestratorStopped
    \/ StopOrchestratorCompleted
    \/ ResumeOrchestratorRunning
    \/ ResumeOrchestratorStopped
    \/ ResumeOrchestratorCompleted
    \/ DestroyOrchestratorRunning
    \/ DestroyOrchestratorStopped
    \/ DestroyOrchestratorCompleted
    \/ ForceCancelMemberRunning
    \/ MemberPeerExposedRunning
    \/ MemberTerminalizedRunning
    \/ OperationPeerTrustedRunning
    \/ PeerInputAdmittedRunning
    \/ BeginCleanupStopped
    \/ BeginCleanupCompleted
    \/ FinishCleanupStopped
    \/ FinishCleanupCompleted
    \/ RunFlowRunning
    \/ StartFlowRunning
    \/ CreateRunRunning
    \/ StartRunRunning
    \/ UnwireRunning
    \/ CompleteFlowRunning
    \/ CompleteFlowRunningZero
    \/ FinishRunRunning
    \/ FinishRunRunningZero
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E agent_identity \in AgentIdentityValues : \E releasing \in OptionSessionIdValues : RetireRunningReleasing(agent_runtime_id, agent_identity, releasing)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E agent_identity \in AgentIdentityValues : \E releasing \in OptionSessionIdValues : RetireRunningPreservingBinding(agent_runtime_id, agent_identity, releasing)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E agent_identity \in AgentIdentityValues : \E releasing \in OptionSessionIdValues : RetireRunningNoBinding(agent_runtime_id, agent_identity, releasing)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E agent_identity \in AgentIdentityValues : \E releasing \in OptionSessionIdValues : RetireStoppedReleasing(agent_runtime_id, agent_identity, releasing)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E agent_identity \in AgentIdentityValues : \E releasing \in OptionSessionIdValues : RetireStoppedPreservingBinding(agent_runtime_id, agent_identity, releasing)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E agent_identity \in AgentIdentityValues : \E releasing \in OptionSessionIdValues : RetireStoppedNoBinding(agent_runtime_id, agent_identity, releasing)
    \/ RetireAllRunning
    \/ RetireAllStopped
    \/ CompleteSpawnRunning
    \/ DestroyFromAny
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : RespawnRunning(agent_runtime_id)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : CancelAllWorkRunning(agent_runtime_id, fence_token)
    \/ TerminalStutter

bindings_require_known_identity == (\A id \in DOMAIN member_realtime_bindings : (id \in DOMAIN identity_to_runtime))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(live_runtime_ids) <= 1 /\ Cardinality(externally_addressable_runtime_ids) <= 1 /\ Cardinality(DOMAIN runtime_fence_tokens) <= 1 /\ Cardinality(DOMAIN member_state_markers) <= 1 /\ Cardinality(wiring_edges) <= 1 /\ Cardinality(DOMAIN identity_to_runtime) <= 1 /\ Cardinality(DOMAIN tasks) <= 1 /\ Cardinality(in_progress_task_ids) <= 1 /\ Cardinality(completed_task_ids) <= 1 /\ Cardinality(DOMAIN member_realtime_bindings) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(live_runtime_ids) <= 2 /\ Cardinality(externally_addressable_runtime_ids) <= 2 /\ Cardinality(DOMAIN runtime_fence_tokens) <= 2 /\ Cardinality(DOMAIN member_state_markers) <= 2 /\ Cardinality(wiring_edges) <= 2 /\ Cardinality(DOMAIN identity_to_runtime) <= 2 /\ Cardinality(DOMAIN tasks) <= 2 /\ Cardinality(in_progress_task_ids) <= 2 /\ Cardinality(completed_task_ids) <= 2 /\ Cardinality(DOMAIN member_realtime_bindings) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []bindings_require_known_identity

=============================================================================
