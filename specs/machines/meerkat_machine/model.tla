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
OptionU64Values == {None} \cup {Some(x) : x \in NatValues}
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

VARIABLES phase, model_step_count, session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase

vars == << phase, model_step_count, session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>

Init ==
    /\ phase = "Initializing"
    /\ model_step_count = 0
    /\ session_id = None
    /\ active_runtime_id = None
    /\ active_fence_token = None
    /\ current_run_id = None
    /\ pre_run_phase = None
    /\ silent_intent_overrides = {}
    /\ realtime_intent_present = FALSE
    /\ realtime_binding_state = "Unbound"
    /\ realtime_binding_authority_epoch = None
    /\ realtime_reattach_required = FALSE
    /\ realtime_next_authority_epoch = 1
    /\ live_topology_phase = "Idle"

TerminalStutter ==
    /\ phase = "Destroyed"
    /\ UNCHANGED vars

Initialize ==
    /\ phase = "Initializing"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


RegisterSessionIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


RegisterSessionAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


RegisterSessionRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


RegisterSessionRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


RegisterSessionStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


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
    /\ UNCHANGED << silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


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
    /\ UNCHANGED << silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


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
    /\ UNCHANGED << silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


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
    /\ UNCHANGED << silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


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
    /\ UNCHANGED << silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


ReconfigureSessionLlmIdentityAttached(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (active_runtime_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


ReconfigureSessionLlmIdentityRunning(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (active_runtime_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


StagePersistentFilterIdle(filter, witnesses) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


StagePersistentFilterAttached(filter, witnesses) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


StagePersistentFilterRunning(filter, witnesses) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


StagePersistentFilterRetired(filter, witnesses) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


StagePersistentFilterStopped(filter, witnesses) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


RequestDeferredToolsIdle(names, witnesses) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


RequestDeferredToolsAttached(names, witnesses) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


RequestDeferredToolsRunning(names, witnesses) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


RequestDeferredToolsRetired(names, witnesses) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


RequestDeferredToolsStopped(names, witnesses) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PrepareBindingsInitializing(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Initializing"
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PrepareBindingsIdle(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Idle"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PrepareBindingsAttached(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PrepareBindingsRunning(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PrepareBindingsRetired(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PrepareBindingsStopped(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


SetPeerIngressContextIdle(keep_alive) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


SetPeerIngressContextAttached(keep_alive) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


SetPeerIngressContextRunning(keep_alive) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


SetPeerIngressContextRetired(keep_alive) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


SetPeerIngressContextStopped(keep_alive) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


NotifyDrainExitedIdle(reason) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


NotifyDrainExitedAttached(reason) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


NotifyDrainExitedRunning(reason) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


NotifyDrainExitedRetired(reason) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


NotifyDrainExitedStopped(reason) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


InterruptCurrentRunAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


InterruptCurrentRun ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


CancelAfterBoundaryAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


CancelAfterBoundary ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


BoundaryAppliedPublish(revision) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PublishCommittedVisibleSetIdle(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PublishCommittedVisibleSetAttached(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PublishCommittedVisibleSetRunning(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PublishCommittedVisibleSetRetired(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PublishCommittedVisibleSetStopped(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


RetireRequestedFromIdle ==
    /\ phase = "Idle" \/ phase = "Attached" \/ phase = "Running"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


Reset ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Attached" \/ phase = "Retired"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ active_fence_token' = None
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


StopRuntimeExecutorUnbound ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Retired"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


StopRuntimeExecutorAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


StopRuntimeExecutorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


Destroy ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Attached" \/ phase = "Running" \/ phase = "Retired" \/ phase = "Stopped"
    /\ (active_runtime_id # None)
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


EnsureSessionWithExecutorIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


EnsureSessionWithExecutorAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


EnsureSessionWithExecutorRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


EnsureSessionWithExecutorRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


EnsureSessionWithExecutorStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


SetSilentIntentsIdle(arg_session_id, intents) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


SetSilentIntentsAttached(arg_session_id, intents) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


SetSilentIntentsRunning(arg_session_id, intents) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


SetSilentIntentsRetired(arg_session_id, intents) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


SetSilentIntentsStopped(arg_session_id, intents) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


AbortIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


AbortAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


AbortRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


AbortRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


AbortStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


WaitIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


WaitAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


WaitRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


WaitRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


WaitStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


AbortAllIdle ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


AbortAllAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


AbortAllRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


AbortAllRetired ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


AbortAllStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


EnsureDrainRunningAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


EnsureDrainRunningRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


IngestIdle(runtime_id, work_id, origin) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


IngestAttached(runtime_id, work_id, origin) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


IngestRunning(runtime_id, work_id, origin) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PublishEventIdle(kind) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PublishEventAttached(kind) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PublishEventRunning(kind) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PublishEventRetired(kind) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PublishEventStopped(kind) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


AcceptWithCompletionIdleQueued(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


AcceptWithCompletionIdleImmediate(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (request_immediate_processing = TRUE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


AcceptWithCompletionAttachedImmediate(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (request_immediate_processing = TRUE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("attached")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


AcceptWithCompletionAttachedQueued(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


AcceptWithCompletionRunningQueuedPassive(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


AcceptWithCompletionRunningInterruptYielding(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


AcceptWithCompletionRunningImmediate(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = TRUE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


AcceptWithoutWakeIdle(input_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


AcceptWithoutWakeAttached(input_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


AcceptWithoutWakeRunning(input_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


ClassifyExternalEnvelopeAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


ClassifyExternalEnvelopeRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


ClassifyPlainEventAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


ClassifyPlainEventRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PrepareIdle(arg_session_id, run_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("idle")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PrepareAttached(arg_session_id, run_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("attached")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


DrainQueuedRunRetired(run_id) ==
    /\ phase = "Retired"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("retired")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


StartConversationRunAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


StartImmediateAppendAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


StartImmediateContextAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


CommitRunningToIdle(input_id, run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("idle"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


CommitRunningToAttached(input_id, run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("attached"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


CommitRunningToRetired(input_id, run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("retired"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


FailRunningToIdle(run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("idle"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


FailRunningToAttached(run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("attached"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


FailRunningToRetired(run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("retired"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


StageAddAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


StageAddRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


StageRemoveAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


StageRemoveRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


StageReloadAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


StageReloadRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


ApplySurfaceBoundaryAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


ApplySurfaceBoundaryRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PendingSucceededAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PendingSucceededRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PendingFailedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


PendingFailedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


CallStartedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


CallStartedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


CallFinishedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


CallFinishedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


FinalizeRemovalCleanAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


FinalizeRemovalCleanRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


FinalizeRemovalForcedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


FinalizeRemovalForcedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


SnapshotAlignedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


SnapshotAlignedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


ShutdownSurfaceAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


ShutdownSurfaceRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


RecycleFromIdleOrRetired ==
    /\ phase = "Idle" \/ phase = "Retired"
    /\ (active_runtime_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ active_fence_token' = None
    /\ current_run_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


RecycleFromAttached ==
    /\ phase = "Attached"
    /\ (active_runtime_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_fence_token' = None
    /\ current_run_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


ProjectRealtimeIntentIdle(present) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_intent_present' = present
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


ProjectRealtimeIntentAttached(present) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_intent_present' = present
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


ProjectRealtimeIntentRunning(present) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_intent_present' = present
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


ProjectRealtimeIntentRetired(present) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_intent_present' = present
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


ProjectRealtimeIntentStopped(present) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_intent_present' = present
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase >>


BeginRealtimeBindingIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "BindingNotReady"
    /\ realtime_binding_authority_epoch' = Some(realtime_next_authority_epoch)
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase >>


BeginRealtimeBindingAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "BindingNotReady"
    /\ realtime_binding_authority_epoch' = Some(realtime_next_authority_epoch)
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase >>


BeginRealtimeBindingRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "BindingNotReady"
    /\ realtime_binding_authority_epoch' = Some(realtime_next_authority_epoch)
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase >>


BeginRealtimeBindingRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "BindingNotReady"
    /\ realtime_binding_authority_epoch' = Some(realtime_next_authority_epoch)
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase >>


BeginRealtimeBindingStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "BindingNotReady"
    /\ realtime_binding_authority_epoch' = Some(realtime_next_authority_epoch)
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase >>


ReplaceRealtimeBindingIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "ReplacementPending"
    /\ realtime_binding_authority_epoch' = Some(realtime_next_authority_epoch)
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase >>


ReplaceRealtimeBindingAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "ReplacementPending"
    /\ realtime_binding_authority_epoch' = Some(realtime_next_authority_epoch)
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase >>


ReplaceRealtimeBindingRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "ReplacementPending"
    /\ realtime_binding_authority_epoch' = Some(realtime_next_authority_epoch)
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase >>


ReplaceRealtimeBindingRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "ReplacementPending"
    /\ realtime_binding_authority_epoch' = Some(realtime_next_authority_epoch)
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase >>


ReplaceRealtimeBindingStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "ReplacementPending"
    /\ realtime_binding_authority_epoch' = Some(realtime_next_authority_epoch)
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase >>


DetachRealtimeBindingIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase >>


DetachRealtimeBindingAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase >>


DetachRealtimeBindingRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase >>


DetachRealtimeBindingRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase >>


DetachRealtimeBindingStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase >>


RequireRealtimeReattachIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase >>


RequireRealtimeReattachAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase >>


RequireRealtimeReattachRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase >>


RequireRealtimeReattachRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase >>


RequireRealtimeReattachStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase >>


PublishRealtimeSignalIdle(authority_epoch, next_binding_state) ==
    /\ phase = "Idle"
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ ((next_binding_state = "BindingNotReady") \/ (next_binding_state = "BindingReady") \/ (next_binding_state = "ReplacementPending"))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = next_binding_state
    /\ realtime_reattach_required' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_authority_epoch, realtime_next_authority_epoch, live_topology_phase >>


PublishRealtimeSignalAttached(authority_epoch, next_binding_state) ==
    /\ phase = "Attached"
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ ((next_binding_state = "BindingNotReady") \/ (next_binding_state = "BindingReady") \/ (next_binding_state = "ReplacementPending"))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = next_binding_state
    /\ realtime_reattach_required' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_authority_epoch, realtime_next_authority_epoch, live_topology_phase >>


PublishRealtimeSignalRunning(authority_epoch, next_binding_state) ==
    /\ phase = "Running"
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ ((next_binding_state = "BindingNotReady") \/ (next_binding_state = "BindingReady") \/ (next_binding_state = "ReplacementPending"))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = next_binding_state
    /\ realtime_reattach_required' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_authority_epoch, realtime_next_authority_epoch, live_topology_phase >>


PublishRealtimeSignalRetired(authority_epoch, next_binding_state) ==
    /\ phase = "Retired"
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ ((next_binding_state = "BindingNotReady") \/ (next_binding_state = "BindingReady") \/ (next_binding_state = "ReplacementPending"))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = next_binding_state
    /\ realtime_reattach_required' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_authority_epoch, realtime_next_authority_epoch, live_topology_phase >>


PublishRealtimeSignalStopped(authority_epoch, next_binding_state) ==
    /\ phase = "Stopped"
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ ((next_binding_state = "BindingNotReady") \/ (next_binding_state = "BindingReady") \/ (next_binding_state = "ReplacementPending"))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = next_binding_state
    /\ realtime_reattach_required' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_authority_epoch, realtime_next_authority_epoch, live_topology_phase >>


BeginLiveTopologyReconfigureIdle(authority_epoch) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Reconfiguring"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


BeginLiveTopologyReconfigureAttached(authority_epoch) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Reconfiguring"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


BeginLiveTopologyReconfigureRunning(authority_epoch) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Reconfiguring"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


BeginLiveTopologyReconfigureRetired(authority_epoch) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Reconfiguring"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


BeginLiveTopologyReconfigureStopped(authority_epoch) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Reconfiguring"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


MarkLiveTopologyDetachedIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ (current_run_id = None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ live_topology_phase' = "Detached"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present >>


MarkLiveTopologyDetachedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ (current_run_id = None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ live_topology_phase' = "Detached"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present >>


MarkLiveTopologyDetachedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ (current_run_id = None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ live_topology_phase' = "Detached"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present >>


MarkLiveTopologyDetachedRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ (current_run_id = None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ live_topology_phase' = "Detached"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present >>


MarkLiveTopologyDetachedStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ (current_run_id = None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ live_topology_phase' = "Detached"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present >>


ApplyLiveTopologyIdentityIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (live_topology_phase = "Detached")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostIdentityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


ApplyLiveTopologyIdentityAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (live_topology_phase = "Detached")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostIdentityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


ApplyLiveTopologyIdentityRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (live_topology_phase = "Detached")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostIdentityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


ApplyLiveTopologyIdentityRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (live_topology_phase = "Detached")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostIdentityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


ApplyLiveTopologyIdentityStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (live_topology_phase = "Detached")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostIdentityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


ApplyLiveTopologyVisibilityIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostIdentityApplied")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostVisibilityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


ApplyLiveTopologyVisibilityAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostIdentityApplied")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostVisibilityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


ApplyLiveTopologyVisibilityRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostIdentityApplied")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostVisibilityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


ApplyLiveTopologyVisibilityRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostIdentityApplied")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostVisibilityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


ApplyLiveTopologyVisibilityStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostIdentityApplied")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostVisibilityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


CompleteLiveTopologyIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostVisibilityApplied")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


CompleteLiveTopologyAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostVisibilityApplied")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


CompleteLiveTopologyRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostVisibilityApplied")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


CompleteLiveTopologyRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostVisibilityApplied")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


CompleteLiveTopologyStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostVisibilityApplied")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


AbortLiveTopologyBeforeDetachIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


AbortLiveTopologyBeforeDetachAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


AbortLiveTopologyBeforeDetachRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


AbortLiveTopologyBeforeDetachRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


AbortLiveTopologyBeforeDetachStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch >>


FailLiveTopologyAfterDetachIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ ((live_topology_phase = "Detached") \/ (live_topology_phase = "HostIdentityApplied") \/ (live_topology_phase = "HostVisibilityApplied"))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present >>


FailLiveTopologyAfterDetachAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ ((live_topology_phase = "Detached") \/ (live_topology_phase = "HostIdentityApplied") \/ (live_topology_phase = "HostVisibilityApplied"))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present >>


FailLiveTopologyAfterDetachRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ ((live_topology_phase = "Detached") \/ (live_topology_phase = "HostIdentityApplied") \/ (live_topology_phase = "HostVisibilityApplied"))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present >>


FailLiveTopologyAfterDetachRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ ((live_topology_phase = "Detached") \/ (live_topology_phase = "HostIdentityApplied") \/ (live_topology_phase = "HostVisibilityApplied"))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present >>


FailLiveTopologyAfterDetachStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ ((live_topology_phase = "Detached") \/ (live_topology_phase = "HostIdentityApplied") \/ (live_topology_phase = "HostVisibilityApplied"))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present >>


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
    \/ \E present \in BOOLEAN : ProjectRealtimeIntentIdle(present)
    \/ \E present \in BOOLEAN : ProjectRealtimeIntentAttached(present)
    \/ \E present \in BOOLEAN : ProjectRealtimeIntentRunning(present)
    \/ \E present \in BOOLEAN : ProjectRealtimeIntentRetired(present)
    \/ \E present \in BOOLEAN : ProjectRealtimeIntentStopped(present)
    \/ BeginRealtimeBindingIdle
    \/ BeginRealtimeBindingAttached
    \/ BeginRealtimeBindingRunning
    \/ BeginRealtimeBindingRetired
    \/ BeginRealtimeBindingStopped
    \/ ReplaceRealtimeBindingIdle
    \/ ReplaceRealtimeBindingAttached
    \/ ReplaceRealtimeBindingRunning
    \/ ReplaceRealtimeBindingRetired
    \/ ReplaceRealtimeBindingStopped
    \/ DetachRealtimeBindingIdle
    \/ DetachRealtimeBindingAttached
    \/ DetachRealtimeBindingRunning
    \/ DetachRealtimeBindingRetired
    \/ DetachRealtimeBindingStopped
    \/ RequireRealtimeReattachIdle
    \/ RequireRealtimeReattachAttached
    \/ RequireRealtimeReattachRunning
    \/ RequireRealtimeReattachRetired
    \/ RequireRealtimeReattachStopped
    \/ \E authority_epoch \in 0..2 : \E next_binding_state \in StringValues : PublishRealtimeSignalIdle(authority_epoch, next_binding_state)
    \/ \E authority_epoch \in 0..2 : \E next_binding_state \in StringValues : PublishRealtimeSignalAttached(authority_epoch, next_binding_state)
    \/ \E authority_epoch \in 0..2 : \E next_binding_state \in StringValues : PublishRealtimeSignalRunning(authority_epoch, next_binding_state)
    \/ \E authority_epoch \in 0..2 : \E next_binding_state \in StringValues : PublishRealtimeSignalRetired(authority_epoch, next_binding_state)
    \/ \E authority_epoch \in 0..2 : \E next_binding_state \in StringValues : PublishRealtimeSignalStopped(authority_epoch, next_binding_state)
    \/ \E authority_epoch \in 0..2 : BeginLiveTopologyReconfigureIdle(authority_epoch)
    \/ \E authority_epoch \in 0..2 : BeginLiveTopologyReconfigureAttached(authority_epoch)
    \/ \E authority_epoch \in 0..2 : BeginLiveTopologyReconfigureRunning(authority_epoch)
    \/ \E authority_epoch \in 0..2 : BeginLiveTopologyReconfigureRetired(authority_epoch)
    \/ \E authority_epoch \in 0..2 : BeginLiveTopologyReconfigureStopped(authority_epoch)
    \/ MarkLiveTopologyDetachedIdle
    \/ MarkLiveTopologyDetachedAttached
    \/ MarkLiveTopologyDetachedRunning
    \/ MarkLiveTopologyDetachedRetired
    \/ MarkLiveTopologyDetachedStopped
    \/ ApplyLiveTopologyIdentityIdle
    \/ ApplyLiveTopologyIdentityAttached
    \/ ApplyLiveTopologyIdentityRunning
    \/ ApplyLiveTopologyIdentityRetired
    \/ ApplyLiveTopologyIdentityStopped
    \/ ApplyLiveTopologyVisibilityIdle
    \/ ApplyLiveTopologyVisibilityAttached
    \/ ApplyLiveTopologyVisibilityRunning
    \/ ApplyLiveTopologyVisibilityRetired
    \/ ApplyLiveTopologyVisibilityStopped
    \/ CompleteLiveTopologyIdle
    \/ CompleteLiveTopologyAttached
    \/ CompleteLiveTopologyRunning
    \/ CompleteLiveTopologyRetired
    \/ CompleteLiveTopologyStopped
    \/ AbortLiveTopologyBeforeDetachIdle
    \/ AbortLiveTopologyBeforeDetachAttached
    \/ AbortLiveTopologyBeforeDetachRunning
    \/ AbortLiveTopologyBeforeDetachRetired
    \/ AbortLiveTopologyBeforeDetachStopped
    \/ FailLiveTopologyAfterDetachIdle
    \/ FailLiveTopologyAfterDetachAttached
    \/ FailLiveTopologyAfterDetachRunning
    \/ FailLiveTopologyAfterDetachRetired
    \/ FailLiveTopologyAfterDetachStopped
    \/ TerminalStutter

fence_requires_bound_runtime == ((active_fence_token = None) \/ (active_runtime_id # None))
running_has_current_run == ((phase # "Running") \/ (current_run_id # None))
current_run_only_while_running_or_retired == ((current_run_id = None) \/ (phase = "Running") \/ (phase = "Retired"))
realtime_binding_epoch_consistency == ((realtime_binding_state = "Unbound") = (realtime_binding_authority_epoch = None))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(silent_intent_overrides) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(silent_intent_overrides) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []fence_requires_bound_runtime
THEOREM Spec => []running_has_current_run
THEOREM Spec => []current_run_only_while_running_or_retired
THEOREM Spec => []realtime_binding_epoch_consistency

=============================================================================
