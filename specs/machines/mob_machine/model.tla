---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MobMachine.

CONSTANTS AgentIdentityValues, AgentRuntimeIdValues, BooleanValues, FenceTokenValues, GenerationValues, MobIdValues, MobMemberStateValues, MobTaskValues, NatValues, SessionIdValues, SetOfAgentRuntimeIdValues, SetOfTaskIdValues, SetOfWiringEdgeValues, StringValues, TaskIdValues, TaskStatusValues, WiringEdgeValues, WorkIdValues, WorkOriginValues

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

VARIABLES phase, model_step_count, live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch

vars == << phase, model_step_count, live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>

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
    /\ member_session_bindings = [x \in {} |-> None]
    /\ pending_session_ingress_detach_runtime_ids = {}
    /\ topology_epoch = 0

TerminalStutter ==
    /\ phase = "Destroyed"
    /\ UNCHANGED vars

WireMembersRunning(edge) ==
    /\ phase = "Running"
    /\ ((edge \in wiring_edges) = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ wiring_edges' = (wiring_edges \cup {edge})
    /\ topology_epoch' = (topology_epoch) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids >>


UnwireMembersRunning(edge) ==
    /\ phase = "Running"
    /\ ((edge \in wiring_edges) = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ wiring_edges' = (wiring_edges \ {edge})
    /\ topology_epoch' = (topology_epoch) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids >>


BindMemberSessionRunning(agent_identity, session_id) ==
    /\ phase = "Running"
    /\ ((agent_identity \in DOMAIN identity_to_runtime) = TRUE)
    /\ ((agent_identity \in DOMAIN member_session_bindings) = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ member_session_bindings' = MapSet(member_session_bindings, agent_identity, session_id)
    /\ topology_epoch' = (topology_epoch) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, pending_session_ingress_detach_runtime_ids >>


RotateMemberSessionRunning(agent_identity, old_session_id, new_session_id) ==
    /\ phase = "Running"
    /\ ((agent_identity \in DOMAIN identity_to_runtime) = TRUE)
    /\ ((agent_identity \in DOMAIN member_session_bindings) = TRUE)
    /\ ((IF (agent_identity \in DOMAIN member_session_bindings) THEN Some((IF agent_identity \in DOMAIN member_session_bindings THEN member_session_bindings[agent_identity] ELSE "None")) ELSE None) = Some(old_session_id))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ member_session_bindings' = MapSet(member_session_bindings, agent_identity, new_session_id)
    /\ topology_epoch' = (topology_epoch) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, pending_session_ingress_detach_runtime_ids >>


ReleaseMemberSessionRunning(agent_identity, session_id) ==
    /\ phase = "Running"
    /\ ((agent_identity \in DOMAIN member_session_bindings) = TRUE)
    /\ ((IF (agent_identity \in DOMAIN member_session_bindings) THEN Some((IF agent_identity \in DOMAIN member_session_bindings THEN member_session_bindings[agent_identity] ELSE "None")) ELSE None) = Some(session_id))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ member_session_bindings' = MapRemove(member_session_bindings, agent_identity)
    /\ topology_epoch' = (topology_epoch) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, pending_session_ingress_detach_runtime_ids >>


SpawnRunningFresh(agent_identity, agent_runtime_id, fence_token, generation, external_addressable, bridge_session_id, replacing) ==
    /\ phase = "Running"
    /\ (coordinator_bound = TRUE)
    /\ ((agent_identity \in DOMAIN member_session_bindings) = FALSE)
    /\ (replacing = None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = (live_runtime_ids \cup {agent_runtime_id})
    /\ externally_addressable_runtime_ids' = IF external_addressable THEN (externally_addressable_runtime_ids \cup {agent_runtime_id}) ELSE (externally_addressable_runtime_ids \ {agent_runtime_id})
    /\ runtime_fence_tokens' = MapSet(runtime_fence_tokens, agent_runtime_id, fence_token)
    /\ identity_to_runtime' = MapSet(identity_to_runtime, agent_identity, agent_runtime_id)
    /\ member_session_bindings' = MapSet(member_session_bindings, agent_identity, bridge_session_id)
    /\ topology_epoch' = (topology_epoch) + 1
    /\ UNCHANGED << active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, tasks, in_progress_task_ids, completed_task_ids, pending_session_ingress_detach_runtime_ids >>


SpawnRunningReplacing(agent_identity, agent_runtime_id, fence_token, generation, external_addressable, bridge_session_id, replacing) ==
    /\ phase = "Running"
    /\ (coordinator_bound = TRUE)
    /\ ((agent_identity \in DOMAIN member_session_bindings) = TRUE)
    /\ (replacing # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = (live_runtime_ids \cup {agent_runtime_id})
    /\ externally_addressable_runtime_ids' = IF external_addressable THEN (externally_addressable_runtime_ids \cup {agent_runtime_id}) ELSE (externally_addressable_runtime_ids \ {agent_runtime_id})
    /\ runtime_fence_tokens' = MapSet(runtime_fence_tokens, agent_runtime_id, fence_token)
    /\ identity_to_runtime' = MapSet(identity_to_runtime, agent_identity, agent_runtime_id)
    /\ member_session_bindings' = MapSet(member_session_bindings, agent_identity, bridge_session_id)
    /\ topology_epoch' = (topology_epoch) + 1
    /\ UNCHANGED << active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, tasks, in_progress_task_ids, completed_task_ids, pending_session_ingress_detach_runtime_ids >>


ObserveRuntimeReady(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


SubmitWorkRunningExternal(agent_runtime_id, fence_token, work_id, origin) ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ (origin = "External")
    /\ (agent_runtime_id \in externally_addressable_runtime_ids)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


SubmitWorkRunningInternal(agent_runtime_id, fence_token, work_id, origin) ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ (origin = "Internal")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


RetireMember(agent_runtime_id, fence_token, session_id) ==
    /\ phase = "Running"
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ member_state_markers' = MapSet(member_state_markers, agent_runtime_id, "Retiring")
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


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
    /\ UNCHANGED << pending_spawn_count, coordinator_bound, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


ResetMember(agent_identity, agent_runtime_id, fence_token, generation, external_addressable, session_id) ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = (live_runtime_ids \cup {agent_runtime_id})
    /\ externally_addressable_runtime_ids' = IF external_addressable THEN (externally_addressable_runtime_ids \cup {agent_runtime_id}) ELSE (externally_addressable_runtime_ids \ {agent_runtime_id})
    /\ runtime_fence_tokens' = MapSet(runtime_fence_tokens, agent_runtime_id, fence_token)
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ identity_to_runtime' = MapSet(identity_to_runtime, agent_identity, agent_runtime_id)
    /\ UNCHANGED << coordinator_bound, member_state_markers, wiring_edges, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


RespawnMember(agent_identity, agent_runtime_id, fence_token, generation, external_addressable, session_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = (live_runtime_ids \cup {agent_runtime_id})
    /\ externally_addressable_runtime_ids' = IF external_addressable THEN (externally_addressable_runtime_ids \cup {agent_runtime_id}) ELSE (externally_addressable_runtime_ids \ {agent_runtime_id})
    /\ runtime_fence_tokens' = MapSet(runtime_fence_tokens, agent_runtime_id, fence_token)
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ identity_to_runtime' = MapSet(identity_to_runtime, agent_identity, agent_runtime_id)
    /\ UNCHANGED << coordinator_bound, member_state_markers, wiring_edges, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


MarkCompleted ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (active_run_count = 0)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


DestroyMob(session_id) ==
    /\ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed"
    /\ (pending_session_ingress_detach_runtime_ids = {})
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = {}
    /\ runtime_fence_tokens' = [x \in {} |-> None]
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ coordinator_bound' = FALSE
    /\ member_state_markers' = [x \in {} |-> None]
    /\ pending_session_ingress_detach_runtime_ids' = {}
    /\ UNCHANGED << externally_addressable_runtime_ids, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, topology_epoch >>


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
    /\ UNCHANGED << externally_addressable_runtime_ids, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


RecordOperatorActionProvenanceRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


RecordOperatorActionProvenanceStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


RecordOperatorActionProvenanceCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


RecordOperatorActionProvenanceDestroyed ==
    /\ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


SetSpawnPolicyRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


SetSpawnPolicyStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


SetSpawnPolicyCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


SetSpawnPolicyDestroyed ==
    /\ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


StopRunning ==
    /\ phase = "Running"
    /\ (active_run_count = 0)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


ResumeStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


CompleteRunning ==
    /\ phase = "Running"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


ResetToRunning ==
    /\ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


TaskCreateRunning(task_id, task_payload) ==
    /\ phase = "Running"
    /\ ((task_id \in DOMAIN tasks) = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ tasks' = MapSet(tasks, task_id, task_payload)
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


TaskUpdateRunningPending(task_id, new_status) ==
    /\ phase = "Running"
    /\ (new_status = "Pending")
    /\ ((task_id \in DOMAIN tasks) = TRUE)
    /\ ((task_id \in completed_task_ids) = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ in_progress_task_ids' = (in_progress_task_ids \ {task_id})
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


TaskUpdateRunningInProgress(task_id, new_status) ==
    /\ phase = "Running"
    /\ (new_status = "InProgress")
    /\ ((task_id \in DOMAIN tasks) = TRUE)
    /\ ((task_id \in completed_task_ids) = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ in_progress_task_ids' = (in_progress_task_ids \cup {task_id})
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


TaskUpdateRunningCompleted(task_id, new_status) ==
    /\ phase = "Running"
    /\ (new_status = "Completed")
    /\ ((task_id \in DOMAIN tasks) = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ in_progress_task_ids' = (in_progress_task_ids \ {task_id})
    /\ completed_task_ids' = (completed_task_ids \cup {task_id})
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


TaskUpdateRunningCancelled(task_id, new_status) ==
    /\ phase = "Running"
    /\ (new_status = "Cancelled")
    /\ ((task_id \in DOMAIN tasks) = TRUE)
    /\ ((task_id \in completed_task_ids) = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ in_progress_task_ids' = (in_progress_task_ids \ {task_id})
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


ForceCancelRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


SubscribeAgentEventsRunning ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


SubscribeAgentEventsStopped ==
    /\ phase = "Stopped"
    /\ (live_runtime_ids # {})
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


SubscribeAgentEventsCompleted ==
    /\ phase = "Completed"
    /\ (live_runtime_ids # {})
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


SubscribeAgentEventsDestroyed ==
    /\ phase = "Destroyed"
    /\ (live_runtime_ids # {})
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


SubscribeAllAgentEventsRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


SubscribeAllAgentEventsStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


SubscribeAllAgentEventsCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


SubscribeAllAgentEventsDestroyed ==
    /\ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


SubscribeMobEventsRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


SubscribeMobEventsStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


SubscribeMobEventsCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


SubscribeMobEventsDestroyed ==
    /\ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


ShutdownRunning ==
    /\ phase = "Running"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


ShutdownStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


ShutdownCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


CancelFlowRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


InitializeOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


BindCoordinatorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


UnbindCoordinatorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


StageSpawnRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_spawn_count' = (pending_spawn_count) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


StopOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


StopOrchestratorStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


StopOrchestratorCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


ResumeOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


ResumeOrchestratorStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


ResumeOrchestratorCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


DestroyOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


DestroyOrchestratorStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


DestroyOrchestratorCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


ForceCancelMemberRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


MemberPeerExposedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


MemberTerminalizedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


OperationPeerTrustedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


PeerInputAdmittedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


BeginCleanupStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


BeginCleanupCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


FinishCleanupStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


FinishCleanupCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


RunFlowRunning ==
    /\ phase = "Running"
    /\ (coordinator_bound = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


StartFlowRunning ==
    /\ phase = "Running"
    /\ (coordinator_bound = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


CreateRunRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


StartRunRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


CompleteFlowRunning ==
    /\ phase = "Running" \/ phase = "Completed"
    /\ (active_run_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) - 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


CompleteFlowRunningZero ==
    /\ phase = "Running" \/ phase = "Completed"
    /\ (active_run_count = 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


FinishRunRunning ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (active_run_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) - 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


FinishRunRunningZero ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (active_run_count = 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


RetireRunningReleasing(agent_runtime_id, agent_identity, releasing, session_id) ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ ((agent_identity \in DOMAIN member_session_bindings) = TRUE)
    /\ (releasing # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ member_state_markers' = MapSet(member_state_markers, agent_runtime_id, "Retiring")
    /\ member_session_bindings' = MapRemove(member_session_bindings, agent_identity)
    /\ pending_session_ingress_detach_runtime_ids' = (pending_session_ingress_detach_runtime_ids \cup {agent_runtime_id})
    /\ topology_epoch' = (topology_epoch) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


RetireRunningPreservingBinding(agent_runtime_id, agent_identity, releasing, session_id) ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ ((agent_identity \in DOMAIN member_session_bindings) = TRUE)
    /\ (releasing = None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ member_state_markers' = MapSet(member_state_markers, agent_runtime_id, "Retiring")
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


RetireRunningNoBinding(agent_runtime_id, agent_identity, releasing, session_id) ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ ((agent_identity \in DOMAIN member_session_bindings) = FALSE)
    /\ (releasing = None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ member_state_markers' = MapSet(member_state_markers, agent_runtime_id, "Retiring")
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


RetireStoppedReleasing(agent_runtime_id, agent_identity, releasing, session_id) ==
    /\ phase = "Stopped"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ ((agent_identity \in DOMAIN member_session_bindings) = TRUE)
    /\ (releasing # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ member_state_markers' = MapSet(member_state_markers, agent_runtime_id, "Retiring")
    /\ member_session_bindings' = MapRemove(member_session_bindings, agent_identity)
    /\ pending_session_ingress_detach_runtime_ids' = (pending_session_ingress_detach_runtime_ids \cup {agent_runtime_id})
    /\ topology_epoch' = (topology_epoch) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids >>


SessionIngressDetachedForMobDestroyRunning(mob_id, agent_runtime_id) ==
    /\ phase = "Running"
    /\ (mob_id = mob_id)
    /\ ((agent_runtime_id \in pending_session_ingress_detach_runtime_ids) = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_session_ingress_detach_runtime_ids' = (pending_session_ingress_detach_runtime_ids \ {agent_runtime_id})
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, topology_epoch >>


SessionIngressDetachedForMobDestroyStopped(mob_id, agent_runtime_id) ==
    /\ phase = "Stopped"
    /\ (mob_id = mob_id)
    /\ ((agent_runtime_id \in pending_session_ingress_detach_runtime_ids) = TRUE)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ pending_session_ingress_detach_runtime_ids' = (pending_session_ingress_detach_runtime_ids \ {agent_runtime_id})
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, topology_epoch >>


SessionIngressDetachFailedForMobDestroyRunning(mob_id, agent_runtime_id, reason) ==
    /\ phase = "Running"
    /\ (mob_id = mob_id)
    /\ (reason = reason)
    /\ ((agent_runtime_id \in pending_session_ingress_detach_runtime_ids) = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


SessionIngressDetachFailedForMobDestroyStopped(mob_id, agent_runtime_id, reason) ==
    /\ phase = "Stopped"
    /\ (mob_id = mob_id)
    /\ (reason = reason)
    /\ ((agent_runtime_id \in pending_session_ingress_detach_runtime_ids) = TRUE)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


RetireStoppedPreservingBinding(agent_runtime_id, agent_identity, releasing, session_id) ==
    /\ phase = "Stopped"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ ((agent_identity \in DOMAIN member_session_bindings) = TRUE)
    /\ (releasing = None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ member_state_markers' = MapSet(member_state_markers, agent_runtime_id, "Retiring")
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


RetireStoppedNoBinding(agent_runtime_id, agent_identity, releasing, session_id) ==
    /\ phase = "Stopped"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ ((agent_identity \in DOMAIN member_session_bindings) = FALSE)
    /\ (releasing = None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ member_state_markers' = MapSet(member_state_markers, agent_runtime_id, "Retiring")
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


RetireAllRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = {}
    /\ runtime_fence_tokens' = [x \in {} |-> None]
    /\ UNCHANGED << externally_addressable_runtime_ids, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


RetireAllStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = {}
    /\ runtime_fence_tokens' = [x \in {} |-> None]
    /\ UNCHANGED << externally_addressable_runtime_ids, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


CompleteSpawnRunning ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (pending_spawn_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_spawn_count' = (pending_spawn_count) - 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


DestroyFromAny ==
    /\ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed"
    /\ (pending_session_ingress_detach_runtime_ids = {})
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = {}
    /\ runtime_fence_tokens' = [x \in {} |-> None]
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ coordinator_bound' = FALSE
    /\ pending_session_ingress_detach_runtime_ids' = {}
    /\ UNCHANGED << externally_addressable_runtime_ids, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, topology_epoch >>


RespawnRunning(agent_runtime_id) ==
    /\ phase = "Running"
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ (coordinator_bound = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


CancelAllWorkRunning(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_state_markers, wiring_edges, identity_to_runtime, tasks, in_progress_task_ids, completed_task_ids, member_session_bindings, pending_session_ingress_detach_runtime_ids, topology_epoch >>


Next ==
    \/ \E edge \in WiringEdgeValues : WireMembersRunning(edge)
    \/ \E edge \in WiringEdgeValues : UnwireMembersRunning(edge)
    \/ \E agent_identity \in AgentIdentityValues : \E session_id \in SessionIdValues : BindMemberSessionRunning(agent_identity, session_id)
    \/ \E agent_identity \in AgentIdentityValues : \E old_session_id \in SessionIdValues : \E new_session_id \in SessionIdValues : RotateMemberSessionRunning(agent_identity, old_session_id, new_session_id)
    \/ \E agent_identity \in AgentIdentityValues : \E session_id \in SessionIdValues : ReleaseMemberSessionRunning(agent_identity, session_id)
    \/ \E agent_identity \in AgentIdentityValues : \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : \E external_addressable \in BOOLEAN : \E bridge_session_id \in SessionIdValues : \E replacing \in OptionSessionIdValues : SpawnRunningFresh(agent_identity, agent_runtime_id, fence_token, generation, external_addressable, bridge_session_id, replacing)
    \/ \E agent_identity \in AgentIdentityValues : \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : \E external_addressable \in BOOLEAN : \E bridge_session_id \in SessionIdValues : \E replacing \in OptionSessionIdValues : SpawnRunningReplacing(agent_identity, agent_runtime_id, fence_token, generation, external_addressable, bridge_session_id, replacing)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : ObserveRuntimeReady(agent_runtime_id, fence_token)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E work_id \in WorkIdValues : \E origin \in WorkOriginValues : SubmitWorkRunningExternal(agent_runtime_id, fence_token, work_id, origin)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E work_id \in WorkIdValues : \E origin \in WorkOriginValues : SubmitWorkRunningInternal(agent_runtime_id, fence_token, work_id, origin)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E session_id \in SessionIdValues : RetireMember(agent_runtime_id, fence_token, session_id)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : ObserveRuntimeRetired(agent_runtime_id, fence_token)
    \/ \E agent_identity \in AgentIdentityValues : \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : \E external_addressable \in BOOLEAN : \E session_id \in SessionIdValues : ResetMember(agent_identity, agent_runtime_id, fence_token, generation, external_addressable, session_id)
    \/ \E agent_identity \in AgentIdentityValues : \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : \E external_addressable \in BOOLEAN : \E session_id \in SessionIdValues : RespawnMember(agent_identity, agent_runtime_id, fence_token, generation, external_addressable, session_id)
    \/ MarkCompleted
    \/ \E session_id \in SessionIdValues : DestroyMob(session_id)
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
    \/ CompleteFlowRunning
    \/ CompleteFlowRunningZero
    \/ FinishRunRunning
    \/ FinishRunRunningZero
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E agent_identity \in AgentIdentityValues : \E releasing \in OptionSessionIdValues : \E session_id \in SessionIdValues : RetireRunningReleasing(agent_runtime_id, agent_identity, releasing, session_id)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E agent_identity \in AgentIdentityValues : \E releasing \in OptionSessionIdValues : \E session_id \in SessionIdValues : RetireRunningPreservingBinding(agent_runtime_id, agent_identity, releasing, session_id)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E agent_identity \in AgentIdentityValues : \E releasing \in OptionSessionIdValues : \E session_id \in SessionIdValues : RetireRunningNoBinding(agent_runtime_id, agent_identity, releasing, session_id)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E agent_identity \in AgentIdentityValues : \E releasing \in OptionSessionIdValues : \E session_id \in SessionIdValues : RetireStoppedReleasing(agent_runtime_id, agent_identity, releasing, session_id)
    \/ \E mob_id \in MobIdValues : \E agent_runtime_id \in AgentRuntimeIdValues : SessionIngressDetachedForMobDestroyRunning(mob_id, agent_runtime_id)
    \/ \E mob_id \in MobIdValues : \E agent_runtime_id \in AgentRuntimeIdValues : SessionIngressDetachedForMobDestroyStopped(mob_id, agent_runtime_id)
    \/ \E mob_id \in MobIdValues : \E agent_runtime_id \in AgentRuntimeIdValues : \E reason \in StringValues : SessionIngressDetachFailedForMobDestroyRunning(mob_id, agent_runtime_id, reason)
    \/ \E mob_id \in MobIdValues : \E agent_runtime_id \in AgentRuntimeIdValues : \E reason \in StringValues : SessionIngressDetachFailedForMobDestroyStopped(mob_id, agent_runtime_id, reason)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E agent_identity \in AgentIdentityValues : \E releasing \in OptionSessionIdValues : \E session_id \in SessionIdValues : RetireStoppedPreservingBinding(agent_runtime_id, agent_identity, releasing, session_id)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E agent_identity \in AgentIdentityValues : \E releasing \in OptionSessionIdValues : \E session_id \in SessionIdValues : RetireStoppedNoBinding(agent_runtime_id, agent_identity, releasing, session_id)
    \/ RetireAllRunning
    \/ RetireAllStopped
    \/ CompleteSpawnRunning
    \/ DestroyFromAny
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : RespawnRunning(agent_runtime_id)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : CancelAllWorkRunning(agent_runtime_id, fence_token)
    \/ TerminalStutter

bindings_require_known_identity == (\A id \in DOMAIN member_session_bindings : (id \in DOMAIN identity_to_runtime))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(live_runtime_ids) <= 1 /\ Cardinality(externally_addressable_runtime_ids) <= 1 /\ Cardinality(DOMAIN runtime_fence_tokens) <= 1 /\ Cardinality(DOMAIN member_state_markers) <= 1 /\ Cardinality(wiring_edges) <= 1 /\ Cardinality(DOMAIN identity_to_runtime) <= 1 /\ Cardinality(DOMAIN tasks) <= 1 /\ Cardinality(in_progress_task_ids) <= 1 /\ Cardinality(completed_task_ids) <= 1 /\ Cardinality(DOMAIN member_session_bindings) <= 1 /\ Cardinality(pending_session_ingress_detach_runtime_ids) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(live_runtime_ids) <= 2 /\ Cardinality(externally_addressable_runtime_ids) <= 2 /\ Cardinality(DOMAIN runtime_fence_tokens) <= 2 /\ Cardinality(DOMAIN member_state_markers) <= 2 /\ Cardinality(wiring_edges) <= 2 /\ Cardinality(DOMAIN identity_to_runtime) <= 2 /\ Cardinality(DOMAIN tasks) <= 2 /\ Cardinality(in_progress_task_ids) <= 2 /\ Cardinality(completed_task_ids) <= 2 /\ Cardinality(DOMAIN member_session_bindings) <= 2 /\ Cardinality(pending_session_ingress_detach_runtime_ids) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []bindings_require_known_identity

=============================================================================
