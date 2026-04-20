---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MobMachine.

CONSTANTS AgentIdentityValues, AgentRuntimeIdValues, BooleanValues, FenceTokenValues, GenerationValues, MobMemberStateValues, MobTaskValues, NatValues, SetOfAgentRuntimeIdValues, SetOfTaskIdValues, SetOfWiringEdgeValues, TaskIdValues, TaskStatusValues, WiringEdgeValues, WorkIdValues, WorkOriginValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

MapAgentIdentityAgentRuntimeIdValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in AgentIdentityValues, v \in AgentRuntimeIdValues }
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

VARIABLES phase, model_step_count, live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids

vars == << phase, model_step_count, live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>

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

TerminalStutter ==
    /\ phase = "Destroyed"
    /\ UNCHANGED vars

SpawnRunning(agent_identity, agent_runtime_id, fence_token, generation, external_addressable) ==
    /\ phase = "Running"
    /\ (coordinator_bound = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = (live_runtime_ids \cup {agent_runtime_id})
    /\ externally_addressable_runtime_ids' = IF external_addressable THEN (externally_addressable_runtime_ids \cup {agent_runtime_id}) ELSE (externally_addressable_runtime_ids \ {agent_runtime_id})
    /\ runtime_fence_tokens' = MapSet(runtime_fence_tokens, agent_runtime_id, fence_token)
    /\ identity_to_runtime' = MapSet(identity_to_runtime, agent_identity, agent_runtime_id)
    /\ UNCHANGED << active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, tasks, in_progress_task_ids, completed_task_ids >>


ObserveRuntimeReady(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


SubmitWorkRunningExternal(agent_runtime_id, fence_token, work_id, origin) ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ (origin = "External")
    /\ (agent_runtime_id \in externally_addressable_runtime_ids)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


SubmitWorkRunningInternal(agent_runtime_id, fence_token, work_id, origin) ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ (origin = "Internal")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


RetireMember(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ member_state_markers' = MapSet(member_state_markers, agent_runtime_id, "Retiring")
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


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
    /\ UNCHANGED << pending_spawn_count, coordinator_bound, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


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
    /\ UNCHANGED << coordinator_bound, member_state_markers, wiring_edges, tasks, in_progress_task_ids, completed_task_ids >>


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
    /\ UNCHANGED << coordinator_bound, member_state_markers, wiring_edges, tasks, in_progress_task_ids, completed_task_ids >>


MarkCompleted ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (active_run_count = 0)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


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
    /\ UNCHANGED << externally_addressable_runtime_ids, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


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
    /\ UNCHANGED << externally_addressable_runtime_ids, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


RecordOperatorActionProvenanceRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


RecordOperatorActionProvenanceStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


RecordOperatorActionProvenanceCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


RecordOperatorActionProvenanceDestroyed ==
    /\ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


SetSpawnPolicyRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


SetSpawnPolicyStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


SetSpawnPolicyCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


SetSpawnPolicyDestroyed ==
    /\ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


StopRunning ==
    /\ phase = "Running"
    /\ (active_run_count = 0)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


ResumeStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


CompleteRunning ==
    /\ phase = "Running"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


ResetToRunning ==
    /\ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


WireRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


TaskCreateRunning(task_id, task_payload) ==
    /\ phase = "Running"
    /\ ((task_id \in DOMAIN tasks) = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ tasks' = MapSet(tasks, task_id, task_payload)
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, in_progress_task_ids, completed_task_ids >>


TaskUpdateRunningPending(task_id, new_status) ==
    /\ phase = "Running"
    /\ (new_status = "Pending")
    /\ ((task_id \in DOMAIN tasks) = TRUE)
    /\ ((task_id \in completed_task_ids) = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ in_progress_task_ids' = (in_progress_task_ids \ {task_id})
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, completed_task_ids >>


TaskUpdateRunningInProgress(task_id, new_status) ==
    /\ phase = "Running"
    /\ (new_status = "InProgress")
    /\ ((task_id \in DOMAIN tasks) = TRUE)
    /\ ((task_id \in completed_task_ids) = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ in_progress_task_ids' = (in_progress_task_ids \cup {task_id})
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, completed_task_ids >>


TaskUpdateRunningCompleted(task_id, new_status) ==
    /\ phase = "Running"
    /\ (new_status = "Completed")
    /\ ((task_id \in DOMAIN tasks) = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ in_progress_task_ids' = (in_progress_task_ids \ {task_id})
    /\ completed_task_ids' = (completed_task_ids \cup {task_id})
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks >>


TaskUpdateRunningCancelled(task_id, new_status) ==
    /\ phase = "Running"
    /\ (new_status = "Cancelled")
    /\ ((task_id \in DOMAIN tasks) = TRUE)
    /\ ((task_id \in completed_task_ids) = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ in_progress_task_ids' = (in_progress_task_ids \ {task_id})
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, completed_task_ids >>


ForceCancelRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


SubscribeAgentEventsRunning ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


SubscribeAgentEventsStopped ==
    /\ phase = "Stopped"
    /\ (live_runtime_ids # {})
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


SubscribeAgentEventsCompleted ==
    /\ phase = "Completed"
    /\ (live_runtime_ids # {})
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


SubscribeAgentEventsDestroyed ==
    /\ phase = "Destroyed"
    /\ (live_runtime_ids # {})
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


SubscribeAllAgentEventsRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


SubscribeAllAgentEventsStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


SubscribeAllAgentEventsCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


SubscribeAllAgentEventsDestroyed ==
    /\ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


SubscribeMobEventsRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


SubscribeMobEventsStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


SubscribeMobEventsCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


SubscribeMobEventsDestroyed ==
    /\ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


ShutdownRunning ==
    /\ phase = "Running"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


ShutdownStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


ShutdownCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


CancelFlowRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


InitializeOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


BindCoordinatorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


UnbindCoordinatorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


StageSpawnRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_spawn_count' = (pending_spawn_count) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


StopOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


StopOrchestratorStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


StopOrchestratorCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


ResumeOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


ResumeOrchestratorStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


ResumeOrchestratorCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


DestroyOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


DestroyOrchestratorStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


DestroyOrchestratorCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


ForceCancelMemberRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


MemberPeerExposedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


MemberTerminalizedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


OperationPeerTrustedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


PeerInputAdmittedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


BeginCleanupStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


BeginCleanupCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


FinishCleanupStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


FinishCleanupCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


RunFlowRunning ==
    /\ phase = "Running"
    /\ (coordinator_bound = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


StartFlowRunning ==
    /\ phase = "Running"
    /\ (coordinator_bound = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


CreateRunRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


StartRunRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


UnwireRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


CompleteFlowRunning ==
    /\ phase = "Running" \/ phase = "Completed"
    /\ (active_run_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) - 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


CompleteFlowRunningZero ==
    /\ phase = "Running" \/ phase = "Completed"
    /\ (active_run_count = 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


FinishRunRunning ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (active_run_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) - 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


FinishRunRunningZero ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (active_run_count = 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


RetireRunning(agent_runtime_id) ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ member_state_markers' = MapSet(member_state_markers, agent_runtime_id, "Retiring")
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


RetireStopped(agent_runtime_id) ==
    /\ phase = "Stopped"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ member_state_markers' = MapSet(member_state_markers, agent_runtime_id, "Retiring")
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


RetireAllRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = {}
    /\ runtime_fence_tokens' = [x \in {} |-> None]
    /\ UNCHANGED << externally_addressable_runtime_ids, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


RetireAllStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = {}
    /\ runtime_fence_tokens' = [x \in {} |-> None]
    /\ UNCHANGED << externally_addressable_runtime_ids, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


CompleteSpawnRunning ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (pending_spawn_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_spawn_count' = (pending_spawn_count) - 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


DestroyFromAny ==
    /\ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = {}
    /\ runtime_fence_tokens' = [x \in {} |-> None]
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << externally_addressable_runtime_ids, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


RespawnRunning(agent_runtime_id) ==
    /\ phase = "Running"
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ (coordinator_bound = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


CancelAllWorkRunning(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


Next ==
    \/ \E agent_identity \in AgentIdentityValues : \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : \E external_addressable \in BOOLEAN : SpawnRunning(agent_identity, agent_runtime_id, fence_token, generation, external_addressable)
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
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : RetireRunning(agent_runtime_id)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : RetireStopped(agent_runtime_id)
    \/ RetireAllRunning
    \/ RetireAllStopped
    \/ CompleteSpawnRunning
    \/ DestroyFromAny
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : RespawnRunning(agent_runtime_id)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : CancelAllWorkRunning(agent_runtime_id, fence_token)
    \/ TerminalStutter


CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(live_runtime_ids) <= 1 /\ Cardinality(externally_addressable_runtime_ids) <= 1 /\ Cardinality(DOMAIN runtime_fence_tokens) <= 1 /\ Cardinality(DOMAIN member_state_markers) <= 1 /\ Cardinality(wiring_edges) <= 1 /\ Cardinality(DOMAIN identity_to_runtime) <= 1 /\ Cardinality(DOMAIN tasks) <= 1 /\ Cardinality(in_progress_task_ids) <= 1 /\ Cardinality(completed_task_ids) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(live_runtime_ids) <= 2 /\ Cardinality(externally_addressable_runtime_ids) <= 2 /\ Cardinality(DOMAIN runtime_fence_tokens) <= 2 /\ Cardinality(DOMAIN member_state_markers) <= 2 /\ Cardinality(wiring_edges) <= 2 /\ Cardinality(DOMAIN identity_to_runtime) <= 2 /\ Cardinality(DOMAIN tasks) <= 2 /\ Cardinality(in_progress_task_ids) <= 2 /\ Cardinality(completed_task_ids) <= 2

Spec == Init /\ [][Next]_vars


=============================================================================
