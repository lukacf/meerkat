---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MobMachine.

CONSTANTS AgentIdentityValues, AgentRuntimeIdValues, BooleanValues, FenceTokenValues, GenerationValues, NatValues, WorkIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionAgentIdentityValues == {None} \cup {Some(x) : x \in AgentIdentityValues}
OptionAgentRuntimeIdValues == {None} \cup {Some(x) : x \in AgentRuntimeIdValues}
OptionFenceTokenValues == {None} \cup {Some(x) : x \in FenceTokenValues}
OptionGenerationValues == {None} \cup {Some(x) : x \in GenerationValues}
OptionWorkIdValues == {None} \cup {Some(x) : x \in WorkIdValues}

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending

vars == << phase, model_step_count, active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>

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
    /\ pending_spawn_count = 0
    /\ retiring_member_count = 0
    /\ wiring_edge_count = 0
    /\ task_count = 0
    /\ event_subscription_count = 0
    /\ active_frame_count = 0
    /\ active_loop_count = 0
    /\ coordinator_bound = FALSE
    /\ kickoff_pending = FALSE

TerminalStutter ==
    /\ phase = "Destroyed"
    /\ UNCHANGED vars

Start ==
    /\ phase = "Creating" \/ phase = "Stopped"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


Spawn(agent_identity, agent_runtime_id, fence_token, generation) ==
    /\ phase = "Creating" \/ phase = "Running" \/ phase = "Stopped"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_identity' = Some(agent_identity)
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ current_generation' = Some(generation)
    /\ inflight_work_id' = None
    /\ active_member_count' = 1
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ retiring_member_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << wiring_edge_count, task_count, event_subscription_count, coordinator_bound >>


ObserveRuntimeReady(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


SubmitWork(agent_runtime_id, fence_token, work_id) ==
    /\ phase = "Running"
    /\ (active_runtime_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = Some(work_id)
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


ObserveWorkCompleted(agent_runtime_id, fence_token, work_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, coordinator_bound >>


ObserveWorkFailed(agent_runtime_id, fence_token, work_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, coordinator_bound >>


ObserveWorkCancelled(agent_runtime_id, fence_token, work_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, coordinator_bound >>


RetireMember(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


ObserveRuntimeRetired(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << active_identity, current_generation, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, coordinator_bound >>


ResetMember(agent_identity, agent_runtime_id, fence_token, generation) ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_identity' = Some(agent_identity)
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ current_generation' = Some(generation)
    /\ inflight_work_id' = None
    /\ active_member_count' = 1
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ retiring_member_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << wiring_edge_count, task_count, event_subscription_count, coordinator_bound >>


RespawnMember(agent_identity, agent_runtime_id, fence_token, generation) ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_identity' = Some(agent_identity)
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ current_generation' = Some(generation)
    /\ inflight_work_id' = None
    /\ active_member_count' = 1
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ retiring_member_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << wiring_edge_count, task_count, event_subscription_count, coordinator_bound >>


MarkCompleted ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (inflight_work_id = None)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


DestroyMob ==
    /\ phase = "Creating" \/ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ active_identity' = None
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ current_generation' = None
    /\ inflight_work_id' = None
    /\ active_member_count' = 0
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ retiring_member_count' = 0
    /\ wiring_edge_count' = 0
    /\ task_count' = 0
    /\ event_subscription_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ coordinator_bound' = FALSE
    /\ kickoff_pending' = FALSE


ObserveRuntimeDestroyed(agent_runtime_id, fence_token) ==
    /\ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed" \/ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ active_identity' = None
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ current_generation' = None
    /\ inflight_work_id' = None
    /\ active_member_count' = 0
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ retiring_member_count' = 0
    /\ wiring_edge_count' = 0
    /\ task_count' = 0
    /\ event_subscription_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ coordinator_bound' = FALSE
    /\ kickoff_pending' = FALSE


FlowStatusRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


McpServerStatesRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


RosterSnapshotRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


ListMembersRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


ListMembersIncludingRetiringRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


ListAllMembersRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


MemberStatusRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


TaskListRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


TaskGetRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


PollEventsRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


ReplayAllEventsRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


RecordOperatorActionProvenanceRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


GetMemberRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


KickoffBarrierSnapshotRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


SetSpawnPolicyRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


CancelFlowRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, coordinator_bound >>


WireRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ wiring_edge_count' = (wiring_edge_count) + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


ExternalTurnRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


InternalTurnRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


StopRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, coordinator_bound >>


ResumeRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


CompleteRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, coordinator_bound >>


ResetRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ retiring_member_count' = 0
    /\ wiring_edge_count' = 0
    /\ task_count' = 0
    /\ event_subscription_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ coordinator_bound' = FALSE
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count >>


TaskCreateRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ task_count' = (task_count) + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


TaskUpdateRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


SubscribeAgentEventsRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ event_subscription_count' = (event_subscription_count) + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


SubscribeAllAgentEventsRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ event_subscription_count' = (event_subscription_count) + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


SubscribeMobEventsRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ event_subscription_count' = (event_subscription_count) + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


InitializeOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, kickoff_pending >>


BindCoordinatorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, kickoff_pending >>


UnbindCoordinatorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, kickoff_pending >>


StageSpawnRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_spawn_count' = (pending_spawn_count) + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


StopOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, kickoff_pending >>


ResumeOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, kickoff_pending >>


DestroyOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, kickoff_pending >>


ForceCancelMemberRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


MemberPeerExposedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


MemberTerminalizedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


OperationPeerTrustedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


PeerInputAdmittedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


RuntimeWorkAdmittedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


KickoffFailedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound >>


KickoffCancelledRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound >>


KickoffForceCancelledRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound >>


RuntimeRunSubmittedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


RuntimeRunCompletedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, coordinator_bound >>


RuntimeRunFailedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, coordinator_bound >>


RuntimeRunCancelledRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, coordinator_bound >>


RuntimeStopRequestedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


DispatchStepRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


CompleteStepRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


RecordStepOutputRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


ConditionPassedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


ConditionRejectedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


FailStepRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


SkipStepRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


ProjectFrameStepStatusRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


CancelStepRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


RegisterTargetsRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


RecordTargetSuccessRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


RecordTargetTerminalFailureRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


RecordTargetCanceledRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


RecordTargetFailureRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


NodeExecutionReleasedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


TerminalizeCompletedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


TerminalizeFailedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


TerminalizeCanceledRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


CompleteNodeRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


RecordNodeOutputRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


FailNodeRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


SkipNodeRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


CancelNodeRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


UntilConditionMetRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


ShutdownRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, coordinator_bound >>


ForceCancelRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, coordinator_bound >>


BeginCleanupRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


FinishCleanupRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


KickoffStartedRunning ==
    /\ phase = "Running"
    /\ (active_member_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ kickoff_pending' = TRUE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound >>


KickoffCallbackPendingRunning ==
    /\ phase = "Running"
    /\ (active_member_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ kickoff_pending' = TRUE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound >>


RunFlowRunning ==
    /\ phase = "Running"
    /\ (active_member_count > 0)
    /\ (active_runtime_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


StartFlowRunning ==
    /\ phase = "Running"
    /\ (active_member_count > 0)
    /\ (active_runtime_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


CreateRunRunning ==
    /\ phase = "Running"
    /\ (active_member_count > 0)
    /\ (active_runtime_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


StartRunRunning ==
    /\ phase = "Running"
    /\ (active_member_count > 0)
    /\ (active_runtime_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


RegisterReadyFrameRunning ==
    /\ phase = "Running"
    /\ (active_run_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_frame_count' = (active_frame_count) + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_loop_count, coordinator_bound, kickoff_pending >>


UnwireRunning ==
    /\ phase = "Running"
    /\ (wiring_edge_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ wiring_edge_count' = (wiring_edge_count) - 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


RegisterPendingBodyFrameRunning ==
    /\ phase = "Running"
    /\ (active_run_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_frame_count' = (active_frame_count) + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_loop_count, coordinator_bound, kickoff_pending >>


CompleteFlowRunning ==
    /\ phase = "Running"
    /\ (active_run_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, coordinator_bound >>


StartRootFrameRunning ==
    /\ phase = "Running"
    /\ (active_run_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_frame_count' = (active_frame_count) + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_loop_count, coordinator_bound, kickoff_pending >>


StartBodyFrameRunning ==
    /\ phase = "Running"
    /\ (active_run_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_frame_count' = (active_frame_count) + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_loop_count, coordinator_bound, kickoff_pending >>


FrameTerminatedRunning ==
    /\ phase = "Running"
    /\ (active_frame_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_frame_count' = (active_frame_count) - 1
    /\ active_loop_count' = 0
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, coordinator_bound, kickoff_pending >>


StartLoopRunning ==
    /\ phase = "Running"
    /\ (active_run_count > 0)
    /\ (active_frame_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_loop_count' = (active_loop_count) + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, coordinator_bound, kickoff_pending >>


BodyFrameStartedRunning ==
    /\ phase = "Running"
    /\ (active_run_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_frame_count' = (active_frame_count) + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_loop_count, coordinator_bound, kickoff_pending >>


BodyFrameCompletedRunning ==
    /\ phase = "Running"
    /\ (active_frame_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_frame_count' = (active_frame_count) - 1
    /\ active_loop_count' = 0
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, coordinator_bound, kickoff_pending >>


BodyFrameFailedRunning ==
    /\ phase = "Running"
    /\ (active_frame_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_frame_count' = (active_frame_count) - 1
    /\ active_loop_count' = 0
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, coordinator_bound, kickoff_pending >>


BodyFrameCanceledRunning ==
    /\ phase = "Running"
    /\ (active_frame_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_frame_count' = (active_frame_count) - 1
    /\ active_loop_count' = 0
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, coordinator_bound, kickoff_pending >>


UntilConditionFailedRunning ==
    /\ phase = "Running"
    /\ (active_loop_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_loop_count' = (active_loop_count) - 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, coordinator_bound, kickoff_pending >>


CancelLoopRunning ==
    /\ phase = "Running"
    /\ (active_loop_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_loop_count' = (active_loop_count) - 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, coordinator_bound, kickoff_pending >>


FinishRunRunning ==
    /\ phase = "Running"
    /\ (active_run_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, coordinator_bound >>


RetireRunning(agent_runtime_id) ==
    /\ phase = "Running"
    /\ (active_member_count > 0)
    /\ (active_member_count > retiring_member_count)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ retiring_member_count' = (retiring_member_count) + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


RetireAllRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ retiring_member_count' = active_member_count
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, pending_spawn_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


CompleteSpawnRunning ==
    /\ phase = "Running"
    /\ (pending_spawn_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_member_count' = (active_member_count) + 1
    /\ pending_spawn_count' = (pending_spawn_count) - 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_run_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


DestroyRunning ==
    /\ phase = "Running"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ active_identity' = None
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ current_generation' = None
    /\ inflight_work_id' = None
    /\ active_member_count' = 0
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ retiring_member_count' = 0
    /\ wiring_edge_count' = 0
    /\ task_count' = 0
    /\ event_subscription_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ coordinator_bound' = FALSE
    /\ kickoff_pending' = FALSE


RespawnRunning(agent_runtime_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_spawn_count' = (pending_spawn_count) + 1
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, inflight_work_id, active_member_count, active_run_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, active_frame_count, active_loop_count, coordinator_bound, kickoff_pending >>


CancelWorkRunning(work_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, coordinator_bound >>


CancelAllWorkRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inflight_work_id' = None
    /\ active_run_count' = 0
    /\ active_frame_count' = 0
    /\ active_loop_count' = 0
    /\ kickoff_pending' = FALSE
    /\ UNCHANGED << active_identity, active_runtime_id, active_fence_token, current_generation, active_member_count, pending_spawn_count, retiring_member_count, wiring_edge_count, task_count, event_subscription_count, coordinator_bound >>


Next ==
    \/ Start
    \/ \E agent_identity \in AgentIdentityValues : \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : Spawn(agent_identity, agent_runtime_id, fence_token, generation)
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
    \/ FlowStatusRunning
    \/ McpServerStatesRunning
    \/ RosterSnapshotRunning
    \/ ListMembersRunning
    \/ ListMembersIncludingRetiringRunning
    \/ ListAllMembersRunning
    \/ MemberStatusRunning
    \/ TaskListRunning
    \/ TaskGetRunning
    \/ PollEventsRunning
    \/ ReplayAllEventsRunning
    \/ RecordOperatorActionProvenanceRunning
    \/ GetMemberRunning
    \/ KickoffBarrierSnapshotRunning
    \/ SetSpawnPolicyRunning
    \/ CancelFlowRunning
    \/ WireRunning
    \/ ExternalTurnRunning
    \/ InternalTurnRunning
    \/ StopRunning
    \/ ResumeRunning
    \/ CompleteRunning
    \/ ResetRunning
    \/ TaskCreateRunning
    \/ TaskUpdateRunning
    \/ SubscribeAgentEventsRunning
    \/ SubscribeAllAgentEventsRunning
    \/ SubscribeMobEventsRunning
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
    \/ RuntimeWorkAdmittedRunning
    \/ KickoffFailedRunning
    \/ KickoffCancelledRunning
    \/ KickoffForceCancelledRunning
    \/ RuntimeRunSubmittedRunning
    \/ RuntimeRunCompletedRunning
    \/ RuntimeRunFailedRunning
    \/ RuntimeRunCancelledRunning
    \/ RuntimeStopRequestedRunning
    \/ DispatchStepRunning
    \/ CompleteStepRunning
    \/ RecordStepOutputRunning
    \/ ConditionPassedRunning
    \/ ConditionRejectedRunning
    \/ FailStepRunning
    \/ SkipStepRunning
    \/ ProjectFrameStepStatusRunning
    \/ CancelStepRunning
    \/ RegisterTargetsRunning
    \/ RecordTargetSuccessRunning
    \/ RecordTargetTerminalFailureRunning
    \/ RecordTargetCanceledRunning
    \/ RecordTargetFailureRunning
    \/ NodeExecutionReleasedRunning
    \/ TerminalizeCompletedRunning
    \/ TerminalizeFailedRunning
    \/ TerminalizeCanceledRunning
    \/ CompleteNodeRunning
    \/ RecordNodeOutputRunning
    \/ FailNodeRunning
    \/ SkipNodeRunning
    \/ CancelNodeRunning
    \/ UntilConditionMetRunning
    \/ ShutdownRunning
    \/ ForceCancelRunning
    \/ BeginCleanupRunning
    \/ FinishCleanupRunning
    \/ KickoffStartedRunning
    \/ KickoffCallbackPendingRunning
    \/ RunFlowRunning
    \/ StartFlowRunning
    \/ CreateRunRunning
    \/ StartRunRunning
    \/ RegisterReadyFrameRunning
    \/ UnwireRunning
    \/ RegisterPendingBodyFrameRunning
    \/ CompleteFlowRunning
    \/ StartRootFrameRunning
    \/ StartBodyFrameRunning
    \/ FrameTerminatedRunning
    \/ StartLoopRunning
    \/ BodyFrameStartedRunning
    \/ BodyFrameCompletedRunning
    \/ BodyFrameFailedRunning
    \/ BodyFrameCanceledRunning
    \/ UntilConditionFailedRunning
    \/ CancelLoopRunning
    \/ FinishRunRunning
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : RetireRunning(agent_runtime_id)
    \/ RetireAllRunning
    \/ CompleteSpawnRunning
    \/ DestroyRunning
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : RespawnRunning(agent_runtime_id)
    \/ \E work_id \in WorkIdValues : CancelWorkRunning(work_id)
    \/ CancelAllWorkRunning
    \/ TerminalStutter

active_work_requires_runtime == ((inflight_work_id = None) \/ (active_runtime_id # None))
destroyed_has_no_active_runtime == ((phase # "Destroyed") \/ (active_runtime_id = None))
active_runtime_has_identity == ((active_runtime_id = None) \/ (active_identity # None))
active_frames_require_runs == ((active_frame_count = 0) \/ (active_run_count > 0))
active_loops_require_frames == ((active_loop_count = 0) \/ (active_frame_count > 0))
retiring_members_do_not_exceed_active_members == (retiring_member_count <= active_member_count)
kickoff_pending_requires_members == ((kickoff_pending = FALSE) \/ (active_member_count > 0))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []active_work_requires_runtime
THEOREM Spec => []destroyed_has_no_active_runtime
THEOREM Spec => []active_runtime_has_identity
THEOREM Spec => []active_frames_require_runs
THEOREM Spec => []active_loops_require_frames
THEOREM Spec => []retiring_members_do_not_exceed_active_members
THEOREM Spec => []kickoff_pending_requires_members

=============================================================================
