---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MeerkatMachine.

CONSTANTS AgentRuntimeIdValues, BooleanValues, FenceTokenValues, GenerationValues, InputIdValues, NatValues, RunIdValues, SessionIdValues, SessionLlmCapabilitySurfaceStatusValues, SessionLlmCapabilitySurfaceValues, SessionLlmIdentityValues, SessionToolVisibilityDeltaValues, SessionToolVisibilityStateValues, SetOfStringValues, StringValues, ToolFilterValues, ToolVisibilityWitnessValues, WorkIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionAgentRuntimeIdValues == {None} \cup {Some(x) : x \in AgentRuntimeIdValues}
OptionFenceTokenValues == {None} \cup {Some(x) : x \in FenceTokenValues}
OptionGenerationValues == {None} \cup {Some(x) : x \in GenerationValues}
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

VARIABLES phase, model_step_count, session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision

vars == << phase, model_step_count, session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>

HasPendingVisibilityPromotion == (staged_visibility_revision > active_visibility_revision)

Init ==
    /\ phase = "Initializing"
    /\ model_step_count = 0
    /\ session_id = None
    /\ active_runtime_id = None
    /\ active_fence_token = None
    /\ active_generation = None
    /\ attachment_live = FALSE
    /\ current_run_id = None
    /\ pre_run_phase = None
    /\ silent_intent_overrides = {}
    /\ staged_requested_deferred_names = {}
    /\ active_visibility_revision = 0
    /\ staged_visibility_revision = 0

TerminalStutter ==
    /\ phase = "Destroyed"
    /\ UNCHANGED vars

RECURSIVE RequestDeferredToolsIdle_ForEach0_staged_requested_deferred_names(_, _)
RequestDeferredToolsIdle_ForEach0_staged_requested_deferred_names(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET name == item IN LET next_acc == (acc \cup {name}) IN RequestDeferredToolsIdle_ForEach0_staged_requested_deferred_names(next_acc, remaining \ {item})

RECURSIVE RequestDeferredToolsAttached_ForEach1_staged_requested_deferred_names(_, _)
RequestDeferredToolsAttached_ForEach1_staged_requested_deferred_names(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET name == item IN LET next_acc == (acc \cup {name}) IN RequestDeferredToolsAttached_ForEach1_staged_requested_deferred_names(next_acc, remaining \ {item})

RECURSIVE RequestDeferredToolsRunning_ForEach2_staged_requested_deferred_names(_, _)
RequestDeferredToolsRunning_ForEach2_staged_requested_deferred_names(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET name == item IN LET next_acc == (acc \cup {name}) IN RequestDeferredToolsRunning_ForEach2_staged_requested_deferred_names(next_acc, remaining \ {item})

RECURSIVE RequestDeferredToolsRetired_ForEach3_staged_requested_deferred_names(_, _)
RequestDeferredToolsRetired_ForEach3_staged_requested_deferred_names(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET name == item IN LET next_acc == (acc \cup {name}) IN RequestDeferredToolsRetired_ForEach3_staged_requested_deferred_names(next_acc, remaining \ {item})

RECURSIVE RequestDeferredToolsStopped_ForEach4_staged_requested_deferred_names(_, _)
RequestDeferredToolsStopped_ForEach4_staged_requested_deferred_names(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET name == item IN LET next_acc == (acc \cup {name}) IN RequestDeferredToolsStopped_ForEach4_staged_requested_deferred_names(next_acc, remaining \ {item})

Initialize ==
    /\ phase = "Initializing"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


RegisterSessionIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


RegisterSessionAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


RegisterSessionRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


RegisterSessionRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


RegisterSessionStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


UnregisterSessionIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ (session_id = Some(arg_session_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = None
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ active_generation' = None
    /\ attachment_live' = FALSE
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ staged_requested_deferred_names' = {}
    /\ active_visibility_revision' = 0
    /\ staged_visibility_revision' = 0
    /\ UNCHANGED << silent_intent_overrides >>


UnregisterSessionAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ (session_id = Some(arg_session_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = None
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ active_generation' = None
    /\ attachment_live' = FALSE
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ staged_requested_deferred_names' = {}
    /\ active_visibility_revision' = 0
    /\ staged_visibility_revision' = 0
    /\ UNCHANGED << silent_intent_overrides >>


UnregisterSessionRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ (session_id = Some(arg_session_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = None
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ active_generation' = None
    /\ attachment_live' = FALSE
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ staged_requested_deferred_names' = {}
    /\ active_visibility_revision' = 0
    /\ staged_visibility_revision' = 0
    /\ UNCHANGED << silent_intent_overrides >>


UnregisterSessionRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ (session_id = Some(arg_session_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = None
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ active_generation' = None
    /\ attachment_live' = FALSE
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ staged_requested_deferred_names' = {}
    /\ active_visibility_revision' = 0
    /\ staged_visibility_revision' = 0
    /\ UNCHANGED << silent_intent_overrides >>


UnregisterSessionStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ (session_id = Some(arg_session_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = None
    /\ active_runtime_id' = None
    /\ active_fence_token' = None
    /\ active_generation' = None
    /\ attachment_live' = FALSE
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ staged_requested_deferred_names' = {}
    /\ active_visibility_revision' = 0
    /\ staged_visibility_revision' = 0
    /\ UNCHANGED << silent_intent_overrides >>


ReconfigureSessionLlmIdentityAttached(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (active_runtime_id # None)
    /\ ((next_active_visibility_revision = active_visibility_revision) \/ (next_active_visibility_revision = ((IF (active_visibility_revision > staged_visibility_revision) THEN active_visibility_revision ELSE staged_visibility_revision) + 1)))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_visibility_revision' = next_active_visibility_revision
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, staged_visibility_revision >>


ReconfigureSessionLlmIdentityRunning(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (active_runtime_id # None)
    /\ ((next_active_visibility_revision = active_visibility_revision) \/ (next_active_visibility_revision = ((IF (active_visibility_revision > staged_visibility_revision) THEN active_visibility_revision ELSE staged_visibility_revision) + 1)))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_visibility_revision' = next_active_visibility_revision
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, staged_visibility_revision >>


StagePersistentFilterIdle(filter, witnesses) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ staged_visibility_revision' = ((IF (active_visibility_revision > staged_visibility_revision) THEN active_visibility_revision ELSE staged_visibility_revision) + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision >>


StagePersistentFilterAttached(filter, witnesses) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ staged_visibility_revision' = ((IF (active_visibility_revision > staged_visibility_revision) THEN active_visibility_revision ELSE staged_visibility_revision) + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision >>


StagePersistentFilterRunning(filter, witnesses) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ staged_visibility_revision' = ((IF (active_visibility_revision > staged_visibility_revision) THEN active_visibility_revision ELSE staged_visibility_revision) + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision >>


StagePersistentFilterRetired(filter, witnesses) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ staged_visibility_revision' = ((IF (active_visibility_revision > staged_visibility_revision) THEN active_visibility_revision ELSE staged_visibility_revision) + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision >>


StagePersistentFilterStopped(filter, witnesses) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ staged_visibility_revision' = ((IF (active_visibility_revision > staged_visibility_revision) THEN active_visibility_revision ELSE staged_visibility_revision) + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision >>


RequestDeferredToolsIdle(names, witnesses) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ staged_requested_deferred_names' = RequestDeferredToolsIdle_ForEach0_staged_requested_deferred_names(staged_requested_deferred_names, names)
    /\ staged_visibility_revision' = ((IF (active_visibility_revision > staged_visibility_revision) THEN active_visibility_revision ELSE staged_visibility_revision) + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, active_visibility_revision >>


RequestDeferredToolsAttached(names, witnesses) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ staged_requested_deferred_names' = RequestDeferredToolsAttached_ForEach1_staged_requested_deferred_names(staged_requested_deferred_names, names)
    /\ staged_visibility_revision' = ((IF (active_visibility_revision > staged_visibility_revision) THEN active_visibility_revision ELSE staged_visibility_revision) + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, active_visibility_revision >>


RequestDeferredToolsRunning(names, witnesses) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ staged_requested_deferred_names' = RequestDeferredToolsRunning_ForEach2_staged_requested_deferred_names(staged_requested_deferred_names, names)
    /\ staged_visibility_revision' = ((IF (active_visibility_revision > staged_visibility_revision) THEN active_visibility_revision ELSE staged_visibility_revision) + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, active_visibility_revision >>


RequestDeferredToolsRetired(names, witnesses) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ staged_requested_deferred_names' = RequestDeferredToolsRetired_ForEach3_staged_requested_deferred_names(staged_requested_deferred_names, names)
    /\ staged_visibility_revision' = ((IF (active_visibility_revision > staged_visibility_revision) THEN active_visibility_revision ELSE staged_visibility_revision) + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, active_visibility_revision >>


RequestDeferredToolsStopped(names, witnesses) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ staged_requested_deferred_names' = RequestDeferredToolsStopped_ForEach4_staged_requested_deferred_names(staged_requested_deferred_names, names)
    /\ staged_visibility_revision' = ((IF (active_visibility_revision > staged_visibility_revision) THEN active_visibility_revision ELSE staged_visibility_revision) + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, active_visibility_revision >>


PrepareBindingsInitializing(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Initializing"
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ active_generation' = Some(generation)
    /\ UNCHANGED << session_id, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


PrepareBindingsIdle(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Idle"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ active_generation' = Some(generation)
    /\ UNCHANGED << session_id, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


PrepareBindingsAttached(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ active_generation' = Some(generation)
    /\ UNCHANGED << session_id, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


PrepareBindingsRunning(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ active_generation' = Some(generation)
    /\ UNCHANGED << session_id, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


PrepareBindingsRetired(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ active_generation' = Some(generation)
    /\ UNCHANGED << session_id, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


PrepareBindingsStopped(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ active_generation' = Some(generation)
    /\ UNCHANGED << session_id, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


SetPeerIngressContextIdle(keep_alive) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


SetPeerIngressContextAttached(keep_alive) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


SetPeerIngressContextRunning(keep_alive) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


SetPeerIngressContextRetired(keep_alive) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


SetPeerIngressContextStopped(keep_alive) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


NotifyDrainExitedIdle(reason) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


NotifyDrainExitedAttached(reason) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


NotifyDrainExitedRunning(reason) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


NotifyDrainExitedRetired(reason) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


NotifyDrainExitedStopped(reason) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


InterruptCurrentRunAttached ==
    /\ phase = "Attached"
    /\ attachment_live
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


InterruptCurrentRun ==
    /\ phase = "Running"
    /\ attachment_live
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


CancelAfterBoundaryAttached ==
    /\ phase = "Attached"
    /\ attachment_live
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


CancelAfterBoundary ==
    /\ phase = "Running"
    /\ attachment_live
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


BoundaryAppliedPromote(revision) ==
    /\ phase = "Running"
    /\ HasPendingVisibilityPromotion
    /\ (staged_visibility_revision = revision)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_visibility_revision' = staged_visibility_revision
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, staged_visibility_revision >>


BoundaryAppliedNoop(revision) ==
    /\ phase = "Running"
    /\ ~(HasPendingVisibilityPromotion)
    /\ (revision <= active_visibility_revision)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


PublishCommittedVisibleSetIdle(active_filter, staged_filter, active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (arg_active_visibility_revision >= arg_staged_visibility_revision)
    /\ ((arg_active_visibility_revision # arg_staged_visibility_revision) \/ (active_filter = staged_filter))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ staged_requested_deferred_names' = arg_staged_requested_deferred_names
    /\ active_visibility_revision' = arg_active_visibility_revision
    /\ staged_visibility_revision' = arg_staged_visibility_revision
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides >>


PublishCommittedVisibleSetAttached(active_filter, staged_filter, active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (arg_active_visibility_revision >= arg_staged_visibility_revision)
    /\ ((arg_active_visibility_revision # arg_staged_visibility_revision) \/ (active_filter = staged_filter))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ staged_requested_deferred_names' = arg_staged_requested_deferred_names
    /\ active_visibility_revision' = arg_active_visibility_revision
    /\ staged_visibility_revision' = arg_staged_visibility_revision
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides >>


PublishCommittedVisibleSetRunning(active_filter, staged_filter, active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (arg_active_visibility_revision >= arg_staged_visibility_revision)
    /\ ((arg_active_visibility_revision # arg_staged_visibility_revision) \/ (active_filter = staged_filter))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ staged_requested_deferred_names' = arg_staged_requested_deferred_names
    /\ active_visibility_revision' = arg_active_visibility_revision
    /\ staged_visibility_revision' = arg_staged_visibility_revision
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides >>


PublishCommittedVisibleSetRetired(active_filter, staged_filter, active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (arg_active_visibility_revision >= arg_staged_visibility_revision)
    /\ ((arg_active_visibility_revision # arg_staged_visibility_revision) \/ (active_filter = staged_filter))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ staged_requested_deferred_names' = arg_staged_requested_deferred_names
    /\ active_visibility_revision' = arg_active_visibility_revision
    /\ staged_visibility_revision' = arg_staged_visibility_revision
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides >>


PublishCommittedVisibleSetStopped(active_filter, staged_filter, active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (arg_active_visibility_revision >= arg_staged_visibility_revision)
    /\ ((arg_active_visibility_revision # arg_staged_visibility_revision) \/ (active_filter = staged_filter))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ staged_requested_deferred_names' = arg_staged_requested_deferred_names
    /\ active_visibility_revision' = arg_active_visibility_revision
    /\ staged_visibility_revision' = arg_staged_visibility_revision
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides >>


RetireRequestedFromIdle ==
    /\ phase = "Idle" \/ phase = "Attached" \/ phase = "Running"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


Reset ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Attached" \/ phase = "Retired"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ active_fence_token' = None
    /\ active_generation' = None
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, attachment_live, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


StopRuntimeExecutorDetached ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Attached" \/ phase = "Running" \/ phase = "Retired"
    /\ ~(attachment_live)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


StopRuntimeExecutorLiveAttached ==
    /\ phase = "Attached"
    /\ attachment_live
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


StopRuntimeExecutorLiveRunning ==
    /\ phase = "Running"
    /\ attachment_live
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


Destroy ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Attached" \/ phase = "Running" \/ phase = "Retired" \/ phase = "Stopped"
    /\ (active_runtime_id # None)
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


EnsureSessionWithExecutorIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ attachment_live' = TRUE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


EnsureSessionWithExecutorAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ attachment_live' = TRUE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


EnsureSessionWithExecutorRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ attachment_live' = TRUE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


EnsureSessionWithExecutorRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


EnsureSessionWithExecutorStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


SetSilentIntentsIdle(arg_session_id, intents) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


SetSilentIntentsAttached(arg_session_id, intents) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


SetSilentIntentsRunning(arg_session_id, intents) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


SetSilentIntentsRetired(arg_session_id, intents) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


SetSilentIntentsStopped(arg_session_id, intents) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


AbortIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


AbortAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


AbortRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


AbortRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


AbortStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


WaitIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


WaitAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


WaitRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


WaitRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


WaitStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


AbortAllIdle ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


AbortAllAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


AbortAllRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


AbortAllRetired ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


AbortAllStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


EnsureDrainRunningAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


EnsureDrainRunningRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


IngestIdle(runtime_id, work_id, origin) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


IngestAttached(runtime_id, work_id, origin) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


IngestRunning(runtime_id, work_id, origin) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


PublishEventIdle(kind) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


PublishEventAttached(kind) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


PublishEventRunning(kind) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


PublishEventRetired(kind) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


PublishEventStopped(kind) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


AcceptWithCompletionIdleQueued(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


AcceptWithCompletionIdleImmediate(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (request_immediate_processing = TRUE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


AcceptWithCompletionAttachedImmediate(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (request_immediate_processing = TRUE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("attached")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


AcceptWithCompletionAttachedQueued(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


AcceptWithCompletionRunningQueuedPassive(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


AcceptWithCompletionRunningInterruptYielding(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


AcceptWithCompletionRunningImmediate(input_id, request_immediate_processing, interrupt_yielding, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = TRUE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


AcceptWithoutWakeIdle(input_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


AcceptWithoutWakeAttached(input_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


AcceptWithoutWakeRunning(input_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


ClassifyExternalEnvelopeAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


ClassifyExternalEnvelopeRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


ClassifyPlainEventAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


ClassifyPlainEventRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


PrepareIdle(arg_session_id, run_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("idle")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


PrepareAttached(arg_session_id, run_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("attached")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


DrainQueuedRunRetired(run_id) ==
    /\ phase = "Retired"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("retired")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


StartConversationRunAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


StartImmediateAppendAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


StartImmediateContextAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


CommitRunningToIdle(input_id, run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("idle"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


FailRunningToIdle(run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("idle"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


CommitRunningToAttached(input_id, run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("attached"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


FailRunningToAttached(run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("attached"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


CommitRunningToRetired(input_id, run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("retired"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


FailRunningToRetired(run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("retired"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


StageAddAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


StageAddRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


StageRemoveAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


StageRemoveRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


StageReloadAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


StageReloadRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


ApplySurfaceBoundaryAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


ApplySurfaceBoundaryRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


PendingSucceededAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


PendingSucceededRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


PendingFailedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


PendingFailedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


CallStartedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


CallStartedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


CallFinishedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


CallFinishedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


FinalizeRemovalCleanAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


FinalizeRemovalCleanRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


FinalizeRemovalForcedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


FinalizeRemovalForcedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


SnapshotAlignedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


SnapshotAlignedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


ShutdownSurfaceAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


ShutdownSurfaceRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, active_generation, attachment_live, current_run_id, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


RecycleFromIdleOrRetired ==
    /\ phase = "Idle" \/ phase = "Retired"
    /\ (active_runtime_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ active_fence_token' = None
    /\ active_generation' = None
    /\ current_run_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, attachment_live, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


RecycleFromAttached ==
    /\ phase = "Attached"
    /\ (active_runtime_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_fence_token' = None
    /\ active_generation' = None
    /\ current_run_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, attachment_live, pre_run_phase, silent_intent_overrides, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision >>


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
    \/ \E revision \in 0..2 : BoundaryAppliedPromote(revision)
    \/ \E revision \in 0..2 : BoundaryAppliedNoop(revision)
    \/ \E active_filter \in ToolFilterValues : \E staged_filter \in ToolFilterValues : \E active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : PublishCommittedVisibleSetIdle(active_filter, staged_filter, active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)
    \/ \E active_filter \in ToolFilterValues : \E staged_filter \in ToolFilterValues : \E active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : PublishCommittedVisibleSetAttached(active_filter, staged_filter, active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)
    \/ \E active_filter \in ToolFilterValues : \E staged_filter \in ToolFilterValues : \E active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : PublishCommittedVisibleSetRunning(active_filter, staged_filter, active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)
    \/ \E active_filter \in ToolFilterValues : \E staged_filter \in ToolFilterValues : \E active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : PublishCommittedVisibleSetRetired(active_filter, staged_filter, active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)
    \/ \E active_filter \in ToolFilterValues : \E staged_filter \in ToolFilterValues : \E active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : PublishCommittedVisibleSetStopped(active_filter, staged_filter, active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)
    \/ RetireRequestedFromIdle
    \/ Reset
    \/ StopRuntimeExecutorDetached
    \/ StopRuntimeExecutorLiveAttached
    \/ StopRuntimeExecutorLiveRunning
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
    \/ \E run_id \in RunIdValues : FailRunningToIdle(run_id)
    \/ \E input_id \in InputIdValues : \E run_id \in RunIdValues : CommitRunningToAttached(input_id, run_id)
    \/ \E run_id \in RunIdValues : FailRunningToAttached(run_id)
    \/ \E input_id \in InputIdValues : \E run_id \in RunIdValues : CommitRunningToRetired(input_id, run_id)
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
current_run_only_while_running_or_retired == ((current_run_id = None) \/ ((phase = "Running") \/ (phase = "Retired")))

CiStateConstraint == /\ model_step_count <= 8 /\ Cardinality(silent_intent_overrides) <= 1 /\ Cardinality(staged_requested_deferred_names) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(silent_intent_overrides) <= 2 /\ Cardinality(staged_requested_deferred_names) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []fence_requires_bound_runtime
THEOREM Spec => []running_has_current_run
THEOREM Spec => []current_run_only_while_running_or_retired

=============================================================================
