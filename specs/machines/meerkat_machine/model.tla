---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MeerkatMachine.

CONSTANTS AgentRuntimeIdValues, BooleanValues, FenceTokenValues, GenerationValues, InputIdValues, NatValues, RunIdValues, SessionIdValues, SessionLlmCapabilitySurfaceStatusValues, SessionLlmCapabilitySurfaceValues, SessionLlmIdentityValues, SessionToolVisibilityDeltaValues, SessionToolVisibilityStateValues, SetOfStringValues, StringValues, ToolFilterValues, ToolVisibilityWitnessValues, WorkIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionAgentRuntimeIdValues == {None} \cup {Some(x) : x \in AgentRuntimeIdValues}
OptionFenceTokenValues == {None} \cup {Some(x) : x \in FenceTokenValues}
OptionRunIdValues == {None} \cup {Some(x) : x \in RunIdValues}
OptionSessionIdValues == {None} \cup {Some(x) : x \in SessionIdValues}
OptionSessionLlmCapabilitySurfaceValues == {None} \cup {Some(x) : x \in SessionLlmCapabilitySurfaceValues}
OptionStringValues == {None} \cup {Some(x) : x \in StringValues}
MapStringToolVisibilityWitnessValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in ToolVisibilityWitnessValues }

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
MapRemove(map, key) == [x \in DOMAIN map \ {key} |-> map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides

vars == << phase, model_step_count, session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>

Init ==
    /\ phase = "Initializing"
    /\ model_step_count = 0
    /\ session_id = None
    /\ active_runtime_id = None
    /\ active_fence_token = None
    /\ current_run_id = None
    /\ pre_run_phase = None
    /\ silent_intent_overrides = {}

TerminalStutter ==
    /\ phase = "Destroyed"
    /\ UNCHANGED vars

Initialize ==
    /\ phase = "Initializing"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


RegisterSessionIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


RegisterSessionAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


RegisterSessionRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


RegisterSessionRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


RegisterSessionStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


UnregisterSessionIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ (session_id = Some(arg_session_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = None
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << silent_intent_overrides >>


UnregisterSessionAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ (session_id = Some(arg_session_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = None
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << silent_intent_overrides >>


UnregisterSessionRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ (session_id = Some(arg_session_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = None
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << silent_intent_overrides >>


UnregisterSessionRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ (session_id = Some(arg_session_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = None
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << silent_intent_overrides >>


UnregisterSessionStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ (session_id = Some(arg_session_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = None
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << silent_intent_overrides >>


ReconfigureSessionLlmIdentityAttached(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (active_runtime_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


ReconfigureSessionLlmIdentityRunning(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (active_runtime_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


StagePersistentFilterIdle(filter, witnesses) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


StagePersistentFilterAttached(filter, witnesses) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


StagePersistentFilterRunning(filter, witnesses) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


StagePersistentFilterRetired(filter, witnesses) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


StagePersistentFilterStopped(filter, witnesses) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


RequestDeferredToolsIdle(names, witnesses) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


RequestDeferredToolsAttached(names, witnesses) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


RequestDeferredToolsRunning(names, witnesses) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


RequestDeferredToolsRetired(names, witnesses) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


RequestDeferredToolsStopped(names, witnesses) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


PrepareBindingsInitializing(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Initializing"
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides >>


PrepareBindingsIdle(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Idle"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides >>


PrepareBindingsAttached(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides >>


PrepareBindingsRunning(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides >>


PrepareBindingsRetired(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides >>


PrepareBindingsStopped(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides >>


SetPeerIngressContextIdle(keep_alive) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


SetPeerIngressContextAttached(keep_alive) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


SetPeerIngressContextRunning(keep_alive) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


SetPeerIngressContextRetired(keep_alive) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


SetPeerIngressContextStopped(keep_alive) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


NotifyDrainExitedIdle(reason) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


NotifyDrainExitedAttached(reason) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


NotifyDrainExitedRunning(reason) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


NotifyDrainExitedRetired(reason) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


NotifyDrainExitedStopped(reason) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


InterruptCurrentRunAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


InterruptCurrentRun ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


CancelAfterBoundaryAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


CancelAfterBoundary ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


BoundaryAppliedPublish(revision) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


PublishCommittedVisibleSetIdle(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


PublishCommittedVisibleSetAttached(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


PublishCommittedVisibleSetRunning(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


PublishCommittedVisibleSetRetired(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


PublishCommittedVisibleSetStopped(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


RetireRequestedFromIdle ==
    /\ phase = "Idle" \/ phase = "Attached" \/ phase = "Running"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


Reset ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Attached" \/ phase = "Retired"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ active_fence_token' = None
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id >>


StopRuntimeExecutorUnbound ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Retired"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token >>


StopRuntimeExecutorAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase >>


StopRuntimeExecutorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase >>


Destroy ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Attached" \/ phase = "Running" \/ phase = "Retired" \/ phase = "Stopped"
    /\ (active_runtime_id # None)
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token >>


EnsureSessionWithExecutorIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


EnsureSessionWithExecutorAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


EnsureSessionWithExecutorRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


EnsureSessionWithExecutorRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


EnsureSessionWithExecutorStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


SetSilentIntentsIdle(arg_session_id, intents) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase >>


SetSilentIntentsAttached(arg_session_id, intents) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase >>


SetSilentIntentsRunning(arg_session_id, intents) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase >>


SetSilentIntentsRetired(arg_session_id, intents) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase >>


SetSilentIntentsStopped(arg_session_id, intents) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


AbortIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


AbortAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


AbortRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


AbortRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


AbortStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


WaitIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


WaitAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


WaitRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


WaitRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


WaitStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


AbortAllIdle ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


AbortAllAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


AbortAllRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


AbortAllRetired ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


AbortAllStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


EnsureDrainRunningAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


EnsureDrainRunningRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


IngestIdle(runtime_id, work_id, origin) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


IngestAttached(runtime_id, work_id, origin) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


IngestRunning(runtime_id, work_id, origin) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


PublishEventIdle(kind) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


PublishEventAttached(kind) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


PublishEventRunning(kind) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


PublishEventRetired(kind) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


PublishEventStopped(kind) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


AcceptWithCompletionIdleQueued(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


AcceptWithCompletionIdleImmediate(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (request_immediate_processing = TRUE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


AcceptWithCompletionAttachedImmediate(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (request_immediate_processing = TRUE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("attached")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides >>


AcceptWithCompletionAttachedQueued(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


AcceptWithCompletionRunningQueuedPassive(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


AcceptWithCompletionRunningInterruptYielding(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


AcceptWithCompletionRunningImmediate(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = TRUE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


AcceptWithoutWakeIdle(input_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


AcceptWithoutWakeAttached(input_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


AcceptWithoutWakeRunning(input_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


ClassifyExternalEnvelopeAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


ClassifyExternalEnvelopeRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


ClassifyPlainEventAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


ClassifyPlainEventRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


PrepareIdle(arg_session_id, run_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("idle")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides >>


PrepareAttached(arg_session_id, run_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("attached")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides >>


DrainQueuedRunRetired(run_id) ==
    /\ phase = "Retired"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("retired")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides >>


StartConversationRunAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


StartImmediateAppendAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


StartImmediateContextAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


CommitRunningToIdle(input_id, run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("idle"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides >>


CommitRunningToAttached(input_id, run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("attached"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides >>


CommitRunningToRetired(input_id, run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("retired"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides >>


FailRunningToIdle(run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("idle"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides >>


FailRunningToAttached(run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("attached"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides >>


FailRunningToRetired(run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("retired"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides >>


StageAddAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


StageAddRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


StageRemoveAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


StageRemoveRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


StageReloadAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


StageReloadRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


ApplySurfaceBoundaryAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


ApplySurfaceBoundaryRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


PendingSucceededAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


PendingSucceededRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


PendingFailedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


PendingFailedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


CallStartedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


CallStartedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


CallFinishedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


CallFinishedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


FinalizeRemovalCleanAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


FinalizeRemovalCleanRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


FinalizeRemovalForcedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


FinalizeRemovalForcedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


SnapshotAlignedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


SnapshotAlignedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


ShutdownSurfaceAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


ShutdownSurfaceRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides >>


RecycleFromIdleOrRetired ==
    /\ phase = "Idle" \/ phase = "Retired"
    /\ (active_runtime_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ active_fence_token' = None
    /\ current_run_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, pre_run_phase, silent_intent_overrides >>


RecycleFromAttached ==
    /\ phase = "Attached"
    /\ (active_runtime_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_fence_token' = None
    /\ current_run_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, pre_run_phase, silent_intent_overrides >>


Next ==
    \/ Initialize
    \/ \E arg_session_id \in SessionIdValues : RegisterSessionIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : RegisterSessionAttached(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : RegisterSessionRunning(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : RegisterSessionRetired(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : RegisterSessionStopped(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : UnregisterSessionIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : UnregisterSessionAttached(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : UnregisterSessionRunning(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : UnregisterSessionRetired(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : UnregisterSessionStopped(arg_session_id)
    \/ \E previous_identity \in SessionLlmIdentityValues : \E previous_visibility_state \in SessionToolVisibilityStateValues : \E previous_capability_surface \in OptionSessionLlmCapabilitySurfaceValues : \E previous_capability_surface_status \in SessionLlmCapabilitySurfaceStatusValues : \E target_identity \in SessionLlmIdentityValues : \E target_capability_surface \in SessionLlmCapabilitySurfaceValues : \E next_visibility_state \in SessionToolVisibilityStateValues : \E next_capability_base_filter \in ToolFilterValues : \E next_active_visibility_revision \in 0..2 : \E tool_visibility_delta \in SessionToolVisibilityDeltaValues : ReconfigureSessionLlmIdentityAttached(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta)
    \/ \E previous_identity \in SessionLlmIdentityValues : \E previous_visibility_state \in SessionToolVisibilityStateValues : \E previous_capability_surface \in OptionSessionLlmCapabilitySurfaceValues : \E previous_capability_surface_status \in SessionLlmCapabilitySurfaceStatusValues : \E target_identity \in SessionLlmIdentityValues : \E target_capability_surface \in SessionLlmCapabilitySurfaceValues : \E next_visibility_state \in SessionToolVisibilityStateValues : \E next_capability_base_filter \in ToolFilterValues : \E next_active_visibility_revision \in 0..2 : \E tool_visibility_delta \in SessionToolVisibilityDeltaValues : ReconfigureSessionLlmIdentityRunning(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta)
    \/ \E filter \in ToolFilterValues : \E witnesses \in MapStringToolVisibilityWitnessValues : StagePersistentFilterIdle(filter, witnesses)
    \/ \E filter \in ToolFilterValues : \E witnesses \in MapStringToolVisibilityWitnessValues : StagePersistentFilterAttached(filter, witnesses)
    \/ \E filter \in ToolFilterValues : \E witnesses \in MapStringToolVisibilityWitnessValues : StagePersistentFilterRunning(filter, witnesses)
    \/ \E filter \in ToolFilterValues : \E witnesses \in MapStringToolVisibilityWitnessValues : StagePersistentFilterRetired(filter, witnesses)
    \/ \E filter \in ToolFilterValues : \E witnesses \in MapStringToolVisibilityWitnessValues : StagePersistentFilterStopped(filter, witnesses)
    \/ \E names \in SetOfStringValues : \E witnesses \in MapStringToolVisibilityWitnessValues : RequestDeferredToolsIdle(names, witnesses)
    \/ \E names \in SetOfStringValues : \E witnesses \in MapStringToolVisibilityWitnessValues : RequestDeferredToolsAttached(names, witnesses)
    \/ \E names \in SetOfStringValues : \E witnesses \in MapStringToolVisibilityWitnessValues : RequestDeferredToolsRunning(names, witnesses)
    \/ \E names \in SetOfStringValues : \E witnesses \in MapStringToolVisibilityWitnessValues : RequestDeferredToolsRetired(names, witnesses)
    \/ \E names \in SetOfStringValues : \E witnesses \in MapStringToolVisibilityWitnessValues : RequestDeferredToolsStopped(names, witnesses)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : PrepareBindingsInitializing(agent_runtime_id, fence_token, generation)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : PrepareBindingsIdle(agent_runtime_id, fence_token, generation)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : PrepareBindingsAttached(agent_runtime_id, fence_token, generation)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : PrepareBindingsRunning(agent_runtime_id, fence_token, generation)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : PrepareBindingsRetired(agent_runtime_id, fence_token, generation)
    \/ \E agent_runtime_id \in AgentRuntimeIdValues : \E fence_token \in FenceTokenValues : \E generation \in GenerationValues : PrepareBindingsStopped(agent_runtime_id, fence_token, generation)
    \/ \E keep_alive \in BOOLEAN : SetPeerIngressContextIdle(keep_alive)
    \/ \E keep_alive \in BOOLEAN : SetPeerIngressContextAttached(keep_alive)
    \/ \E keep_alive \in BOOLEAN : SetPeerIngressContextRunning(keep_alive)
    \/ \E keep_alive \in BOOLEAN : SetPeerIngressContextRetired(keep_alive)
    \/ \E keep_alive \in BOOLEAN : SetPeerIngressContextStopped(keep_alive)
    \/ \E reason \in StringValues : NotifyDrainExitedIdle(reason)
    \/ \E reason \in StringValues : NotifyDrainExitedAttached(reason)
    \/ \E reason \in StringValues : NotifyDrainExitedRunning(reason)
    \/ \E reason \in StringValues : NotifyDrainExitedRetired(reason)
    \/ \E reason \in StringValues : NotifyDrainExitedStopped(reason)
    \/ InterruptCurrentRunAttached
    \/ InterruptCurrentRun
    \/ CancelAfterBoundaryAttached
    \/ CancelAfterBoundary
    \/ \E revision \in 0..2 : BoundaryAppliedPublish(revision)
    \/ \E active_filter \in ToolFilterValues : \E staged_filter \in ToolFilterValues : \E active_requested_deferred_names \in SetOfStringValues : \E staged_requested_deferred_names \in SetOfStringValues : \E active_visibility_revision \in 0..2 : \E staged_visibility_revision \in 0..2 : PublishCommittedVisibleSetIdle(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision)
    \/ \E active_filter \in ToolFilterValues : \E staged_filter \in ToolFilterValues : \E active_requested_deferred_names \in SetOfStringValues : \E staged_requested_deferred_names \in SetOfStringValues : \E active_visibility_revision \in 0..2 : \E staged_visibility_revision \in 0..2 : PublishCommittedVisibleSetAttached(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision)
    \/ \E active_filter \in ToolFilterValues : \E staged_filter \in ToolFilterValues : \E active_requested_deferred_names \in SetOfStringValues : \E staged_requested_deferred_names \in SetOfStringValues : \E active_visibility_revision \in 0..2 : \E staged_visibility_revision \in 0..2 : PublishCommittedVisibleSetRunning(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision)
    \/ \E active_filter \in ToolFilterValues : \E staged_filter \in ToolFilterValues : \E active_requested_deferred_names \in SetOfStringValues : \E staged_requested_deferred_names \in SetOfStringValues : \E active_visibility_revision \in 0..2 : \E staged_visibility_revision \in 0..2 : PublishCommittedVisibleSetRetired(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision)
    \/ \E active_filter \in ToolFilterValues : \E staged_filter \in ToolFilterValues : \E active_requested_deferred_names \in SetOfStringValues : \E staged_requested_deferred_names \in SetOfStringValues : \E active_visibility_revision \in 0..2 : \E staged_visibility_revision \in 0..2 : PublishCommittedVisibleSetStopped(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision)
    \/ RetireRequestedFromIdle
    \/ Reset
    \/ StopRuntimeExecutorUnbound
    \/ StopRuntimeExecutorAttached
    \/ StopRuntimeExecutorRunning
    \/ Destroy
    \/ \E arg_session_id \in SessionIdValues : EnsureSessionWithExecutorIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : EnsureSessionWithExecutorAttached(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : EnsureSessionWithExecutorRunning(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : EnsureSessionWithExecutorRetired(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : EnsureSessionWithExecutorStopped(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : \E intents \in SetOfStringValues : SetSilentIntentsIdle(arg_session_id, intents)
    \/ \E arg_session_id \in SessionIdValues : \E intents \in SetOfStringValues : SetSilentIntentsAttached(arg_session_id, intents)
    \/ \E arg_session_id \in SessionIdValues : \E intents \in SetOfStringValues : SetSilentIntentsRunning(arg_session_id, intents)
    \/ \E arg_session_id \in SessionIdValues : \E intents \in SetOfStringValues : SetSilentIntentsRetired(arg_session_id, intents)
    \/ \E arg_session_id \in SessionIdValues : \E intents \in SetOfStringValues : SetSilentIntentsStopped(arg_session_id, intents)
    \/ \E arg_session_id \in SessionIdValues : AbortIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : AbortAttached(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : AbortRunning(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : AbortRetired(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : AbortStopped(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : WaitIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : WaitAttached(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : WaitRunning(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : WaitRetired(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : WaitStopped(arg_session_id)
    \/ AbortAllIdle
    \/ AbortAllAttached
    \/ AbortAllRunning
    \/ AbortAllRetired
    \/ AbortAllStopped
    \/ EnsureDrainRunningAttached
    \/ EnsureDrainRunningRunning
    \/ \E runtime_id \in AgentRuntimeIdValues : \E work_id \in WorkIdValues : \E origin \in StringValues : IngestIdle(runtime_id, work_id, origin)
    \/ \E runtime_id \in AgentRuntimeIdValues : \E work_id \in WorkIdValues : \E origin \in StringValues : IngestAttached(runtime_id, work_id, origin)
    \/ \E runtime_id \in AgentRuntimeIdValues : \E work_id \in WorkIdValues : \E origin \in StringValues : IngestRunning(runtime_id, work_id, origin)
    \/ \E kind \in StringValues : PublishEventIdle(kind)
    \/ \E kind \in StringValues : PublishEventAttached(kind)
    \/ \E kind \in StringValues : PublishEventRunning(kind)
    \/ \E kind \in StringValues : PublishEventRetired(kind)
    \/ \E kind \in StringValues : PublishEventStopped(kind)
    \/ \E input_id \in InputIdValues : \E request_immediate_processing \in BOOLEAN : \E interrupt_yielding \in BOOLEAN : \E run_id \in RunIdValues : AcceptWithCompletionIdleQueued(input_id, request_immediate_processing, interrupt_yielding, run_id)
    \/ \E input_id \in InputIdValues : \E request_immediate_processing \in BOOLEAN : \E interrupt_yielding \in BOOLEAN : \E run_id \in RunIdValues : AcceptWithCompletionIdleImmediate(input_id, request_immediate_processing, interrupt_yielding, run_id)
    \/ \E input_id \in InputIdValues : \E request_immediate_processing \in BOOLEAN : \E interrupt_yielding \in BOOLEAN : \E run_id \in RunIdValues : AcceptWithCompletionAttachedImmediate(input_id, request_immediate_processing, interrupt_yielding, run_id)
    \/ \E input_id \in InputIdValues : \E request_immediate_processing \in BOOLEAN : \E interrupt_yielding \in BOOLEAN : \E run_id \in RunIdValues : AcceptWithCompletionAttachedQueued(input_id, request_immediate_processing, interrupt_yielding, run_id)
    \/ \E input_id \in InputIdValues : \E request_immediate_processing \in BOOLEAN : \E interrupt_yielding \in BOOLEAN : \E run_id \in RunIdValues : AcceptWithCompletionRunningQueuedPassive(input_id, request_immediate_processing, interrupt_yielding, run_id)
    \/ \E input_id \in InputIdValues : \E request_immediate_processing \in BOOLEAN : \E interrupt_yielding \in BOOLEAN : \E run_id \in RunIdValues : AcceptWithCompletionRunningInterruptYielding(input_id, request_immediate_processing, interrupt_yielding, run_id)
    \/ \E input_id \in InputIdValues : \E request_immediate_processing \in BOOLEAN : \E interrupt_yielding \in BOOLEAN : \E run_id \in RunIdValues : AcceptWithCompletionRunningImmediate(input_id, request_immediate_processing, interrupt_yielding, run_id)
    \/ \E input_id \in InputIdValues : AcceptWithoutWakeIdle(input_id)
    \/ \E input_id \in InputIdValues : AcceptWithoutWakeAttached(input_id)
    \/ \E input_id \in InputIdValues : AcceptWithoutWakeRunning(input_id)
    \/ ClassifyExternalEnvelopeAttached
    \/ ClassifyExternalEnvelopeRunning
    \/ ClassifyPlainEventAttached
    \/ ClassifyPlainEventRunning
    \/ \E arg_session_id \in SessionIdValues : \E run_id \in RunIdValues : PrepareIdle(arg_session_id, run_id)
    \/ \E arg_session_id \in SessionIdValues : \E run_id \in RunIdValues : PrepareAttached(arg_session_id, run_id)
    \/ \E run_id \in RunIdValues : DrainQueuedRunRetired(run_id)
    \/ StartConversationRunAttached
    \/ StartImmediateAppendAttached
    \/ StartImmediateContextAttached
    \/ \E input_id \in InputIdValues : \E run_id \in RunIdValues : CommitRunningToIdle(input_id, run_id)
    \/ \E input_id \in InputIdValues : \E run_id \in RunIdValues : CommitRunningToAttached(input_id, run_id)
    \/ \E input_id \in InputIdValues : \E run_id \in RunIdValues : CommitRunningToRetired(input_id, run_id)
    \/ \E run_id \in RunIdValues : FailRunningToIdle(run_id)
    \/ \E run_id \in RunIdValues : FailRunningToAttached(run_id)
    \/ \E run_id \in RunIdValues : FailRunningToRetired(run_id)
    \/ StageAddAttached
    \/ StageAddRunning
    \/ StageRemoveAttached
    \/ StageRemoveRunning
    \/ StageReloadAttached
    \/ StageReloadRunning
    \/ ApplySurfaceBoundaryAttached
    \/ ApplySurfaceBoundaryRunning
    \/ PendingSucceededAttached
    \/ PendingSucceededRunning
    \/ PendingFailedAttached
    \/ PendingFailedRunning
    \/ CallStartedAttached
    \/ CallStartedRunning
    \/ CallFinishedAttached
    \/ CallFinishedRunning
    \/ FinalizeRemovalCleanAttached
    \/ FinalizeRemovalCleanRunning
    \/ FinalizeRemovalForcedAttached
    \/ FinalizeRemovalForcedRunning
    \/ SnapshotAlignedAttached
    \/ SnapshotAlignedRunning
    \/ ShutdownSurfaceAttached
    \/ ShutdownSurfaceRunning
    \/ RecycleFromIdleOrRetired
    \/ RecycleFromAttached
    \/ TerminalStutter

fence_requires_bound_runtime == ((active_fence_token = None) \/ (active_runtime_id # None))
running_has_current_run == ((phase # "Running") \/ (current_run_id # None))
current_run_only_while_running_or_retired == ((current_run_id = None) \/ (phase = "Running") \/ (phase = "Retired"))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(silent_intent_overrides) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(silent_intent_overrides) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []fence_requires_bound_runtime
THEOREM Spec => []running_has_current_run
THEOREM Spec => []current_run_only_while_running_or_retired

=============================================================================
