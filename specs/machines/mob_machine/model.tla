---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MobMachine.

CONSTANTS AgentIdentityValues, AgentRuntimeIdValues, BooleanValues, FenceTokenValues, GenerationValues, NatValues, SetOfAgentIdentityValues, SetOfAgentRuntimeIdValues, StringValues, WorkIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

MapAgentRuntimeIdFenceTokenValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in AgentRuntimeIdValues, v \in FenceTokenValues }

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
MapRemove(map, key) == [x \in DOMAIN map \ {key} |-> map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent

vars == << phase, model_step_count, live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>

Init ==
    /\ phase = "Running"
    /\ model_step_count = 0
    /\ live_runtime_ids = {}
    /\ externally_addressable_runtime_ids = {}
    /\ runtime_fence_tokens = [x \in {} |-> None]
    /\ active_run_count = 0
    /\ pending_spawn_count = 0
    /\ coordinator_bound = TRUE
    /\ member_voice_intent = {}

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
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ UNCHANGED << coordinator_bound, member_voice_intent >>


ObserveRuntimeReady(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


SubmitWorkRunningExternal(agent_runtime_id, fence_token, work_id, origin) ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ (origin = "External")
    /\ (agent_runtime_id \in externally_addressable_runtime_ids)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


SubmitWorkRunningInternal(agent_runtime_id, fence_token, work_id, origin) ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ (origin = "Internal")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


RetireMember(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


ObserveRuntimeRetired(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = (live_runtime_ids \ {agent_runtime_id})
    /\ externally_addressable_runtime_ids' = (externally_addressable_runtime_ids \ {agent_runtime_id})
    /\ runtime_fence_tokens' = MapRemove(runtime_fence_tokens, agent_runtime_id)
    /\ active_run_count' = 0
    /\ UNCHANGED << pending_spawn_count, coordinator_bound, member_voice_intent >>


ResetMember(agent_identity, agent_runtime_id, fence_token, generation, external_addressable) ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = (live_runtime_ids \cup {agent_runtime_id})
    /\ externally_addressable_runtime_ids' = IF external_addressable THEN (externally_addressable_runtime_ids \cup {agent_runtime_id}) ELSE (externally_addressable_runtime_ids \ {agent_runtime_id})
    /\ runtime_fence_tokens' = MapSet(runtime_fence_tokens, agent_runtime_id, fence_token)
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ UNCHANGED << coordinator_bound, member_voice_intent >>


RespawnMember(agent_identity, agent_runtime_id, fence_token, generation, external_addressable) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = (live_runtime_ids \cup {agent_runtime_id})
    /\ externally_addressable_runtime_ids' = IF external_addressable THEN (externally_addressable_runtime_ids \cup {agent_runtime_id}) ELSE (externally_addressable_runtime_ids \ {agent_runtime_id})
    /\ runtime_fence_tokens' = MapSet(runtime_fence_tokens, agent_runtime_id, fence_token)
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ UNCHANGED << coordinator_bound, member_voice_intent >>


MarkCompleted ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (active_run_count = 0)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


DestroyMob ==
    /\ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = {}
    /\ runtime_fence_tokens' = [x \in {} |-> None]
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << externally_addressable_runtime_ids, member_voice_intent >>


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
    /\ UNCHANGED << externally_addressable_runtime_ids, member_voice_intent >>


RecordOperatorActionProvenanceRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


RecordOperatorActionProvenanceStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


RecordOperatorActionProvenanceCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


RecordOperatorActionProvenanceDestroyed ==
    /\ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


SetSpawnPolicyRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


SetSpawnPolicyStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


SetSpawnPolicyCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


SetSpawnPolicyDestroyed ==
    /\ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


StopRunning ==
    /\ phase = "Running"
    /\ (active_run_count = 0)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, member_voice_intent >>


ResumeStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_voice_intent >>


CompleteRunning ==
    /\ phase = "Running"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_voice_intent >>


ResetToRunning ==
    /\ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, member_voice_intent >>


WireRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


ExternalTurnRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


InternalTurnRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


TaskCreateRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


TaskUpdateRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


ForceCancelRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_voice_intent >>


SubscribeAgentEventsRunning ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


SubscribeAgentEventsStopped ==
    /\ phase = "Stopped"
    /\ (live_runtime_ids # {})
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


SubscribeAgentEventsCompleted ==
    /\ phase = "Completed"
    /\ (live_runtime_ids # {})
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


SubscribeAgentEventsDestroyed ==
    /\ phase = "Destroyed"
    /\ (live_runtime_ids # {})
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


SubscribeAllAgentEventsRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


SubscribeAllAgentEventsStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


SubscribeAllAgentEventsCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


SubscribeAllAgentEventsDestroyed ==
    /\ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


SubscribeMobEventsRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


SubscribeMobEventsStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


SubscribeMobEventsCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


SubscribeMobEventsDestroyed ==
    /\ phase = "Destroyed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


ShutdownRunning ==
    /\ phase = "Running"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, member_voice_intent >>


ShutdownStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, member_voice_intent >>


ShutdownCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, member_voice_intent >>


CancelFlowRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_voice_intent >>


InitializeOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_voice_intent >>


BindCoordinatorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_voice_intent >>


UnbindCoordinatorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_voice_intent >>


StageSpawnRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_spawn_count' = (pending_spawn_count) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, coordinator_bound, member_voice_intent >>


StopOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_voice_intent >>


StopOrchestratorStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_voice_intent >>


StopOrchestratorCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_voice_intent >>


ResumeOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_voice_intent >>


ResumeOrchestratorStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_voice_intent >>


ResumeOrchestratorCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = TRUE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_voice_intent >>


DestroyOrchestratorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_voice_intent >>


DestroyOrchestratorStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_voice_intent >>


DestroyOrchestratorCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, member_voice_intent >>


ForceCancelMemberRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


MemberPeerExposedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


MemberTerminalizedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


OperationPeerTrustedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


PeerInputAdmittedRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


BeginCleanupStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


BeginCleanupCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


FinishCleanupStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


FinishCleanupCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


RunFlowRunning ==
    /\ phase = "Running"
    /\ (coordinator_bound = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_voice_intent >>


StartFlowRunning ==
    /\ phase = "Running"
    /\ (coordinator_bound = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_voice_intent >>


CreateRunRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_voice_intent >>


StartRunRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = (active_run_count) + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_voice_intent >>


UnwireRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


CompleteFlowRunning ==
    /\ phase = "Running" \/ phase = "Completed"
    /\ (active_run_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_voice_intent >>


FinishRunRunning ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (active_run_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_voice_intent >>


RetireRunning(agent_runtime_id) ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = (live_runtime_ids \ {agent_runtime_id})
    /\ runtime_fence_tokens' = MapRemove(runtime_fence_tokens, agent_runtime_id)
    /\ UNCHANGED << externally_addressable_runtime_ids, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


RetireStopped(agent_runtime_id) ==
    /\ phase = "Stopped"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = (live_runtime_ids \ {agent_runtime_id})
    /\ runtime_fence_tokens' = MapRemove(runtime_fence_tokens, agent_runtime_id)
    /\ UNCHANGED << externally_addressable_runtime_ids, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


RetireAllRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = {}
    /\ runtime_fence_tokens' = [x \in {} |-> None]
    /\ UNCHANGED << externally_addressable_runtime_ids, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


RetireAllStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = {}
    /\ runtime_fence_tokens' = [x \in {} |-> None]
    /\ UNCHANGED << externally_addressable_runtime_ids, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


CompleteSpawnRunning ==
    /\ phase = "Running" \/ phase = "Stopped"
    /\ (pending_spawn_count > 0)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_spawn_count' = (pending_spawn_count) - 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, coordinator_bound, member_voice_intent >>


DestroyFromAny ==
    /\ phase = "Running" \/ phase = "Stopped" \/ phase = "Completed"
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ live_runtime_ids' = {}
    /\ runtime_fence_tokens' = [x \in {} |-> None]
    /\ active_run_count' = 0
    /\ pending_spawn_count' = 0
    /\ coordinator_bound' = FALSE
    /\ UNCHANGED << externally_addressable_runtime_ids, member_voice_intent >>


RespawnRunning(agent_runtime_id) ==
    /\ phase = "Running"
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ (coordinator_bound = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound, member_voice_intent >>


CancelAllWorkRunning(agent_runtime_id, fence_token) ==
    /\ phase = "Running"
    /\ (live_runtime_ids # {})
    /\ (agent_runtime_id \in live_runtime_ids)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_run_count' = 0
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, pending_spawn_count, coordinator_bound, member_voice_intent >>


RealtimeAttach(agent_identity) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ member_voice_intent' = (member_voice_intent \cup {agent_identity})
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound >>


RealtimeDetach(agent_identity) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ member_voice_intent' = (member_voice_intent \ {agent_identity})
    /\ UNCHANGED << live_runtime_ids, externally_addressable_runtime_ids, runtime_fence_tokens, active_run_count, pending_spawn_count, coordinator_bound >>


Next ==
    \/ \E agent_identity \in AgentIdentityValues : \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : \E external_addressable \in BOOLEAN : SpawnRunning(agent_identity, agent_runtime_id, fence_token, generation, external_addressable)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : ObserveRuntimeReady(agent_runtime_id, fence_token)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E work_id \in WorkIdValues : \E origin \in StringValues : SubmitWorkRunningExternal(agent_runtime_id, fence_token, work_id, origin)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E work_id \in WorkIdValues : \E origin \in StringValues : SubmitWorkRunningInternal(agent_runtime_id, fence_token, work_id, origin)
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
    \/ FinishRunRunning
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : RetireRunning(agent_runtime_id)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : RetireStopped(agent_runtime_id)
    \/ RetireAllRunning
    \/ RetireAllStopped
    \/ CompleteSpawnRunning
    \/ DestroyFromAny
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : RespawnRunning(agent_runtime_id)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : CancelAllWorkRunning(agent_runtime_id, fence_token)
    \/ \E agent_identity \in AgentIdentityValues : RealtimeAttach(agent_identity)
    \/ \E agent_identity \in AgentIdentityValues : RealtimeDetach(agent_identity)
    \/ TerminalStutter


CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(live_runtime_ids) <= 1 /\ Cardinality(externally_addressable_runtime_ids) <= 1 /\ Cardinality(DOMAIN runtime_fence_tokens) <= 1 /\ Cardinality(member_voice_intent) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(live_runtime_ids) <= 2 /\ Cardinality(externally_addressable_runtime_ids) <= 2 /\ Cardinality(DOMAIN runtime_fence_tokens) <= 2 /\ Cardinality(member_voice_intent) <= 2

Spec == Init /\ [][Next]_vars


=============================================================================
