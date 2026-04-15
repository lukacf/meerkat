---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MobMachine.

CONSTANTS AgentIdentityValues, AgentRuntimeIdValues, BooleanValues, FenceTokenValues, GenerationValues, NatValues, WorkIdValues

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

VARIABLES phase, model_step_count, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound

vars == << phase, model_step_count, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>

Init ==
    /\ phase = "Running"
    /\ model_step_count = 0
    /\ active_member_count = 0
    /\ active_run_count = 0
    /\ pending_spawn_count = 0
    /\ retiring_member_count = 0
    /\ wiring_edge_count = 0
    /\ coordinator_bound = FALSE

TerminalStutter ==
    /\ phase = "Destroyed"
    /\ UNCHANGED vars

SpawnRunning(agent_identity, agent_runtime_id, fence_token, generation) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_member_count' = 1
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ retiring_member_count' = 0
    /\ UNCHANGED << wiring_edge_count, coordinator_bound >>


ObserveRuntimeReady(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


SubmitWorkRunning(agent_runtime_id, fence_token, work_id) ==
    /\ phase = "Running"
    /\ (active_member_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


ObserveWorkCompleted(agent_runtime_id, fence_token, work_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


ObserveWorkFailed(agent_runtime_id, fence_token, work_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


ObserveWorkCancelled(agent_runtime_id, fence_token, work_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


RetireMember(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


ObserveRuntimeRetired(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


ResetMember(agent_identity, agent_runtime_id, fence_token, generation) ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_member_count' = 1
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ retiring_member_count' = 0
    /\ UNCHANGED << wiring_edge_count, coordinator_bound >>


RespawnMember(agent_identity, agent_runtime_id, fence_token, generation) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_member_count' = 1
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ retiring_member_count' = 0
    /\ UNCHANGED << wiring_edge_count, coordinator_bound >>


MarkCompleted ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (active_run_count = 0)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


DestroyMob ==
    /\ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ active_member_count' = 0
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ retiring_member_count' = 0
    /\ wiring_edge_count' = 0
    /\ coordinator_bound' = FALSE


ObserveRuntimeDestroyed(agent_runtime_id, fence_token) ==
    /\ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed" \/ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ active_member_count' = 0
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ retiring_member_count' = 0
    /\ wiring_edge_count' = 0
    /\ coordinator_bound' = FALSE


RecordOperatorActionProvenanceRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


RecordOperatorActionProvenanceStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


RecordOperatorActionProvenanceCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


RecordOperatorActionProvenanceDestroyed ==
    /\ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


SetSpawnPolicyRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


SetSpawnPolicyStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


SetSpawnPolicyCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


SetSpawnPolicyDestroyed ==
    /\ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


StopRunning ==
    /\ phase = "Running"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count >>


ResumeStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count >>


CompleteRunning ==
    /\ phase = "Running"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


ResetToRunning ==
    /\ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ retiring_member_count' = 0
    /\ wiring_edge_count' = 0
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << active_member_count >>


WireRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ wiring_edge_count' = (wiring_edge_count) + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, coordinator_bound >>


ExternalTurnRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


InternalTurnRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


TaskCreateRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


TaskUpdateRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


ForceCancelRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


SubscribeAgentEventsRunning ==
    /\ phase = "Running"
    /\ (active_member_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


SubscribeAgentEventsStopped ==
    /\ phase = "Stopped"
    /\ (active_member_count > 0)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


SubscribeAgentEventsCompleted ==
    /\ phase = "Completed"
    /\ (active_member_count > 0)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


SubscribeAgentEventsDestroyed ==
    /\ phase = "Destroyed"
    /\ (active_member_count > 0)
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


SubscribeAllAgentEventsRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


SubscribeAllAgentEventsStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


SubscribeAllAgentEventsCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


SubscribeAllAgentEventsDestroyed ==
    /\ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


SubscribeMobEventsRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


SubscribeMobEventsStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


SubscribeMobEventsCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


SubscribeMobEventsDestroyed ==
    /\ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


ShutdownRunning ==
    /\ phase = "Running"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count >>


ShutdownStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count >>


ShutdownCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count >>


CancelFlowRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


InitializeOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count >>


BindCoordinatorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count >>


UnbindCoordinatorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count >>


StageSpawnRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_spawn_count' = (pending_spawn_count) + 1
    /\ UNCHANGED << active_member_count, active_run_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


StopOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count >>


ResumeOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count >>


DestroyOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count >>


ForceCancelMemberRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


MemberPeerExposedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


MemberTerminalizedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


OperationPeerTrustedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


PeerInputAdmittedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


BeginCleanupStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


BeginCleanupCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


FinishCleanupStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


FinishCleanupCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


RunFlowRunning ==
    /\ phase = "Running"
    /\ (active_member_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


StartFlowRunning ==
    /\ phase = "Running"
    /\ (active_member_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


CreateRunRunning ==
    /\ phase = "Running"
    /\ (active_member_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


StartRunRunning ==
    /\ phase = "Running"
    /\ (active_member_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


UnwireRunning ==
    /\ phase = "Running"
    /\ (wiring_edge_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ wiring_edge_count' = (wiring_edge_count) - 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, coordinator_bound >>


CompleteFlowRunning ==
    /\ phase = "Running" \/ phase = "Completed"
    /\ (active_run_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


FinishRunRunning ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (active_run_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


RetireRunning(agent_runtime_id) ==
    /\ phase = "Running"
    /\ (active_member_count > 0)
    /\ (active_member_count > retiring_member_count)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_member_count' = (active_member_count) - 1
    /\ retiring_member_count' = 0
    /\ UNCHANGED << active_run_count, pending_spawn_count, wiring_edge_count, coordinator_bound >>


RetireStopped(agent_runtime_id) ==
    /\ phase = "Stopped"
    /\ (active_member_count > 0)
    /\ (active_member_count > retiring_member_count)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_member_count' = (active_member_count) - 1
    /\ retiring_member_count' = 0
    /\ UNCHANGED << active_run_count, pending_spawn_count, wiring_edge_count, coordinator_bound >>


RetireAllRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_member_count' = 0
    /\ retiring_member_count' = 0
    /\ UNCHANGED << active_run_count, pending_spawn_count, wiring_edge_count, coordinator_bound >>


RetireAllStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_member_count' = 0
    /\ retiring_member_count' = 0
    /\ UNCHANGED << active_run_count, pending_spawn_count, wiring_edge_count, coordinator_bound >>


CompleteSpawnRunning ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (pending_spawn_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_member_count' = (active_member_count) + 1
    /\ pending_spawn_count' = (pending_spawn_count) - 1
    /\ UNCHANGED << active_run_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


DestroyFromAny ==
    /\ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ active_member_count' = 0
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ retiring_member_count' = 0
    /\ wiring_edge_count' = 0
    /\ coordinator_bound' = FALSE


RespawnRunning(agent_runtime_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


CancelAllWorkRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, coordinator_bound >>


Next ==
    \/ \E agent_identity \in AgentIdentityValues : \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : SpawnRunning(agent_identity, agent_runtime_id, fence_token, generation)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : ObserveRuntimeReady(agent_runtime_id, fence_token)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E work_id \in WorkIdValues : SubmitWorkRunning(agent_runtime_id, fence_token, work_id)
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
    \/ ExternalTurnRunning
    \/ InternalTurnRunning
    \/ TaskCreateRunning
    \/ TaskUpdateRunning
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
    \/ ResumeOrchestratorRunning
    \/ DestroyOrchestratorRunning
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
    \/ FinishRunRunning
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : RetireRunning(agent_runtime_id)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : RetireStopped(agent_runtime_id)
    \/ RetireAllRunning
    \/ RetireAllStopped
    \/ CompleteSpawnRunning
    \/ DestroyFromAny
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : RespawnRunning(agent_runtime_id)
    \/ CancelAllWorkRunning
    \/ TerminalStutter

retiring_members_do_not_exceed_active_members == (retiring_member_count <= active_member_count)

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []retiring_members_do_not_exceed_active_members

=============================================================================
