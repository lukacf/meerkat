---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MeerkatMachine.

CONSTANTS AgentRuntimeIdValues, BooleanValues, FenceTokenValues, GenerationValues, InboundPeerRequestStateValues, InputIdValues, McpServerIdValues, McpServerStateValues, NatValues, OutboundPeerRequestStateValues, PeerCorrelationIdValues, PeerTerminalDispositionValues, RealtimeBindingStateValues, RunIdValues, SessionIdValues, SessionLlmCapabilitySurfaceStatusValues, SessionLlmCapabilitySurfaceValues, SessionLlmIdentityValues, SessionToolVisibilityDeltaValues, SessionToolVisibilityStateValues, SetOfStringValues, StringValues, ToolFilterValues, ToolVisibilityWitnessValues, WorkIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionAgentRuntimeIdValues == {None} \cup {Some(x) : x \in AgentRuntimeIdValues}
OptionFenceTokenValues == {None} \cup {Some(x) : x \in FenceTokenValues}
OptionRunIdValues == {None} \cup {Some(x) : x \in RunIdValues}
OptionSessionIdValues == {None} \cup {Some(x) : x \in SessionIdValues}
OptionSessionLlmCapabilitySurfaceValues == {None} \cup {Some(x) : x \in SessionLlmCapabilitySurfaceValues}
OptionStringValues == {None} \cup {Some(x) : x \in StringValues}
OptionU64Values == {None} \cup {Some(x) : x \in NatValues}
MapMcpServerIdMcpServerStateValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in McpServerIdValues, v \in McpServerStateValues }
MapPeerCorrelationIdInboundPeerRequestStateValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in PeerCorrelationIdValues, v \in InboundPeerRequestStateValues }
MapPeerCorrelationIdOutboundPeerRequestStateValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in PeerCorrelationIdValues, v \in OutboundPeerRequestStateValues }
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

VARIABLES phase, model_step_count, session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests

vars == << phase, model_step_count, session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>

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
    /\ mcp_server_states = [x \in {} |-> None]
    /\ pending_peer_requests = [x \in {} |-> None]
    /\ inbound_peer_requests = [x \in {} |-> None]

TerminalStutter ==
    /\ phase = "Destroyed"
    /\ UNCHANGED vars

Initialize ==
    /\ phase = "Initializing"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


RegisterSessionIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


RegisterSessionAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


RegisterSessionRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


RegisterSessionRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


RegisterSessionStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ReconfigureSessionLlmIdentityAttached(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (active_runtime_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ReconfigureSessionLlmIdentityRunning(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (active_runtime_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


StagePersistentFilterIdle(filter, witnesses) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


StagePersistentFilterAttached(filter, witnesses) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


StagePersistentFilterRunning(filter, witnesses) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


StagePersistentFilterRetired(filter, witnesses) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


StagePersistentFilterStopped(filter, witnesses) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


RequestDeferredToolsIdle(names, witnesses) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


RequestDeferredToolsAttached(names, witnesses) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


RequestDeferredToolsRunning(names, witnesses) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


RequestDeferredToolsRetired(names, witnesses) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


RequestDeferredToolsStopped(names, witnesses) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PrepareBindingsInitializing(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Initializing"
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PrepareBindingsIdle(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Idle"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PrepareBindingsAttached(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PrepareBindingsRunning(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PrepareBindingsRetired(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PrepareBindingsStopped(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


SetPeerIngressContextIdle(keep_alive) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


SetPeerIngressContextAttached(keep_alive) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


SetPeerIngressContextRunning(keep_alive) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


SetPeerIngressContextRetired(keep_alive) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


SetPeerIngressContextStopped(keep_alive) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


NotifyDrainExitedIdle(reason) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


NotifyDrainExitedAttached(reason) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


NotifyDrainExitedRunning(reason) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


NotifyDrainExitedRetired(reason) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


NotifyDrainExitedStopped(reason) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


InterruptCurrentRunAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


InterruptCurrentRun ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


CancelAfterBoundaryAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


CancelAfterBoundary ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


BoundaryAppliedPublish(revision) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PublishCommittedVisibleSetIdle(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PublishCommittedVisibleSetAttached(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PublishCommittedVisibleSetRunning(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PublishCommittedVisibleSetRetired(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PublishCommittedVisibleSetStopped(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


RetireRequestedFromIdle ==
    /\ phase = "Idle" \/ phase = "Attached" \/ phase = "Running"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


Reset ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Attached" \/ phase = "Retired"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ active_fence_token' = None
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


StopRuntimeExecutorUnbound ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Retired"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


StopRuntimeExecutorAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


StopRuntimeExecutorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


Destroy ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Attached" \/ phase = "Running" \/ phase = "Retired" \/ phase = "Stopped"
    /\ (active_runtime_id # None)
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


EnsureSessionWithExecutorIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


EnsureSessionWithExecutorAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


EnsureSessionWithExecutorRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


EnsureSessionWithExecutorRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


EnsureSessionWithExecutorStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


SetSilentIntentsIdle(arg_session_id, intents) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


SetSilentIntentsAttached(arg_session_id, intents) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


SetSilentIntentsRunning(arg_session_id, intents) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


SetSilentIntentsRetired(arg_session_id, intents) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


SetSilentIntentsStopped(arg_session_id, intents) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AbortIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AbortAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AbortRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AbortRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AbortStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


WaitIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


WaitAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


WaitRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


WaitRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


WaitStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AbortAllIdle ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AbortAllAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AbortAllRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AbortAllRetired ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AbortAllStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


EnsureDrainRunningAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


EnsureDrainRunningRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


IngestIdle(runtime_id, work_id, origin) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


IngestAttached(runtime_id, work_id, origin) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


IngestRunning(runtime_id, work_id, origin) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PublishEventIdle(kind) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PublishEventAttached(kind) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PublishEventRunning(kind) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PublishEventRetired(kind) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PublishEventStopped(kind) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AcceptWithCompletionIdleQueued(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AcceptWithCompletionIdleImmediate(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (request_immediate_processing = TRUE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AcceptWithCompletionAttachedImmediate(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (request_immediate_processing = TRUE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("attached")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AcceptWithCompletionAttachedQueued(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AcceptWithCompletionRunningQueuedPassive(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ (wake_if_idle = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AcceptWithCompletionRunningQueuedWakeIfIdle(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ (wake_if_idle = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AcceptWithCompletionRunningInterruptYielding(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AcceptWithCompletionRunningImmediate(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = TRUE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AcceptWithoutWakeIdle(input_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AcceptWithoutWakeAttached(input_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AcceptWithoutWakeRunning(input_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ClassifyExternalEnvelopeAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ClassifyExternalEnvelopeRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ClassifyPlainEventAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ClassifyPlainEventRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PrepareIdle(arg_session_id, run_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("idle")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PrepareAttached(arg_session_id, run_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("attached")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


DrainQueuedRunRetired(run_id) ==
    /\ phase = "Retired"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("retired")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


StartConversationRunAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


StartImmediateAppendAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


StartImmediateContextAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


CommitRunningToIdle(input_id, run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("idle"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


CommitRunningToAttached(input_id, run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("attached"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


CommitRunningToRetired(input_id, run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("retired"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


FailRunningToIdle(run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("idle"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


FailRunningToAttached(run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("attached"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


FailRunningToRetired(run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("retired"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


StageAddAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


StageAddRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


StageRemoveAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


StageRemoveRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


StageReloadAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


StageReloadRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ApplySurfaceBoundaryAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ApplySurfaceBoundaryRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PendingSucceededAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PendingSucceededRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PendingFailedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PendingFailedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


CallStartedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


CallStartedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


CallFinishedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


CallFinishedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


FinalizeRemovalCleanAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


FinalizeRemovalCleanRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


FinalizeRemovalForcedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


FinalizeRemovalForcedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


SnapshotAlignedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


SnapshotAlignedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ShutdownSurfaceAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ShutdownSurfaceRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


RecycleFromIdleOrRetired ==
    /\ phase = "Idle" \/ phase = "Retired"
    /\ (active_runtime_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ active_fence_token' = None
    /\ current_run_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


RecycleFromAttached ==
    /\ phase = "Attached"
    /\ (active_runtime_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_fence_token' = None
    /\ current_run_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ProjectRealtimeIntentIdle(present) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_intent_present' = present
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ProjectRealtimeIntentAttached(present) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_intent_present' = present
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ProjectRealtimeIntentRunning(present) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_intent_present' = present
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ProjectRealtimeIntentRetired(present) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_intent_present' = present
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ProjectRealtimeIntentStopped(present) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_intent_present' = present
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


DetachRealtimeBindingIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


DetachRealtimeBindingAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


DetachRealtimeBindingRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


DetachRealtimeBindingRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


DetachRealtimeBindingStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


RequireRealtimeReattachIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


RequireRealtimeReattachAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


RequireRealtimeReattachRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


RequireRealtimeReattachRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


RequireRealtimeReattachStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PublishRealtimeSignalIdle(authority_epoch, next_binding_state) ==
    /\ phase = "Idle"
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ ((next_binding_state = "BindingNotReady") \/ (next_binding_state = "BindingReady") \/ (next_binding_state = "ReplacementPending"))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = next_binding_state
    /\ realtime_reattach_required' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_authority_epoch, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PublishRealtimeSignalAttached(authority_epoch, next_binding_state) ==
    /\ phase = "Attached"
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ ((next_binding_state = "BindingNotReady") \/ (next_binding_state = "BindingReady") \/ (next_binding_state = "ReplacementPending"))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = next_binding_state
    /\ realtime_reattach_required' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_authority_epoch, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PublishRealtimeSignalRunning(authority_epoch, next_binding_state) ==
    /\ phase = "Running"
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ ((next_binding_state = "BindingNotReady") \/ (next_binding_state = "BindingReady") \/ (next_binding_state = "ReplacementPending"))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = next_binding_state
    /\ realtime_reattach_required' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_authority_epoch, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PublishRealtimeSignalRetired(authority_epoch, next_binding_state) ==
    /\ phase = "Retired"
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ ((next_binding_state = "BindingNotReady") \/ (next_binding_state = "BindingReady") \/ (next_binding_state = "ReplacementPending"))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = next_binding_state
    /\ realtime_reattach_required' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_authority_epoch, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


PublishRealtimeSignalStopped(authority_epoch, next_binding_state) ==
    /\ phase = "Stopped"
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ ((next_binding_state = "BindingNotReady") \/ (next_binding_state = "BindingReady") \/ (next_binding_state = "ReplacementPending"))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = next_binding_state
    /\ realtime_reattach_required' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_authority_epoch, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


McpServerConnectPendingIdle(server_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerConnectPendingAttached(server_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerConnectPendingRunning(server_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerConnectPendingRetired(server_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerConnectPendingStopped(server_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerConnectedIdle(server_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Connected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerConnectedAttached(server_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Connected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerConnectedRunning(server_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Connected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerConnectedRetired(server_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Connected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerConnectedStopped(server_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Connected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerFailedIdle(server_id, error) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Failed")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerFailedAttached(server_id, error) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Failed")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerFailedRunning(server_id, error) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Failed")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerFailedRetired(server_id, error) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Failed")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerFailedStopped(server_id, error) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Failed")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerDisconnectedIdle(server_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Disconnected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerDisconnectedAttached(server_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Disconnected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerDisconnectedRunning(server_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Disconnected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerDisconnectedRetired(server_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Disconnected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerDisconnectedStopped(server_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Disconnected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerReloadIdle(server_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerReloadAttached(server_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerReloadRunning(server_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerReloadRetired(server_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


McpServerReloadStopped(server_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests >>


PeerRequestSentIdle(corr_id, to) ==
    /\ phase = "Idle"
    /\ ~((corr_id \in DOMAIN pending_peer_requests))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "Sent")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerRequestSentAttached(corr_id, to) ==
    /\ phase = "Attached"
    /\ ~((corr_id \in DOMAIN pending_peer_requests))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "Sent")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerRequestSentRunning(corr_id, to) ==
    /\ phase = "Running"
    /\ ~((corr_id \in DOMAIN pending_peer_requests))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "Sent")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerRequestSentRetired(corr_id, to) ==
    /\ phase = "Retired"
    /\ ~((corr_id \in DOMAIN pending_peer_requests))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "Sent")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerRequestSentStopped(corr_id, to) ==
    /\ phase = "Stopped"
    /\ ~((corr_id \in DOMAIN pending_peer_requests))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "Sent")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerResponseProgressArrivedIdle(corr_id) ==
    /\ phase = "Idle"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "AcceptedProgress")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerResponseProgressArrivedAttached(corr_id) ==
    /\ phase = "Attached"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "AcceptedProgress")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerResponseProgressArrivedRunning(corr_id) ==
    /\ phase = "Running"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "AcceptedProgress")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerResponseProgressArrivedRetired(corr_id) ==
    /\ phase = "Retired"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "AcceptedProgress")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerResponseProgressArrivedStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "AcceptedProgress")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerResponseTerminalArrivedCompletedIdle(corr_id, disposition) ==
    /\ phase = "Idle"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Completed")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerResponseTerminalArrivedCompletedAttached(corr_id, disposition) ==
    /\ phase = "Attached"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Completed")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerResponseTerminalArrivedCompletedRunning(corr_id, disposition) ==
    /\ phase = "Running"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Completed")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerResponseTerminalArrivedCompletedRetired(corr_id, disposition) ==
    /\ phase = "Retired"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Completed")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerResponseTerminalArrivedCompletedStopped(corr_id, disposition) ==
    /\ phase = "Stopped"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Completed")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerResponseTerminalArrivedFailedIdle(corr_id, disposition) ==
    /\ phase = "Idle"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Failed")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerResponseTerminalArrivedFailedAttached(corr_id, disposition) ==
    /\ phase = "Attached"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Failed")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerResponseTerminalArrivedFailedRunning(corr_id, disposition) ==
    /\ phase = "Running"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Failed")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerResponseTerminalArrivedFailedRetired(corr_id, disposition) ==
    /\ phase = "Retired"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Failed")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerResponseTerminalArrivedFailedStopped(corr_id, disposition) ==
    /\ phase = "Stopped"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Failed")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerRequestTimedOutIdle(corr_id) ==
    /\ phase = "Idle"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerRequestTimedOutAttached(corr_id) ==
    /\ phase = "Attached"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerRequestTimedOutRunning(corr_id) ==
    /\ phase = "Running"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerRequestTimedOutRetired(corr_id) ==
    /\ phase = "Retired"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerRequestTimedOutStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests >>


PeerRequestReceivedIdle(corr_id) ==
    /\ phase = "Idle"
    /\ ~((corr_id \in DOMAIN inbound_peer_requests))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapSet(inbound_peer_requests, corr_id, "Received")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests >>


PeerRequestReceivedAttached(corr_id) ==
    /\ phase = "Attached"
    /\ ~((corr_id \in DOMAIN inbound_peer_requests))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapSet(inbound_peer_requests, corr_id, "Received")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests >>


PeerRequestReceivedRunning(corr_id) ==
    /\ phase = "Running"
    /\ ~((corr_id \in DOMAIN inbound_peer_requests))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapSet(inbound_peer_requests, corr_id, "Received")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests >>


PeerRequestReceivedRetired(corr_id) ==
    /\ phase = "Retired"
    /\ ~((corr_id \in DOMAIN inbound_peer_requests))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapSet(inbound_peer_requests, corr_id, "Received")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests >>


PeerRequestReceivedStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ ~((corr_id \in DOMAIN inbound_peer_requests))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapSet(inbound_peer_requests, corr_id, "Received")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests >>


PeerResponseRepliedIdle(corr_id) ==
    /\ phase = "Idle"
    /\ (corr_id \in DOMAIN inbound_peer_requests)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapRemove(inbound_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests >>


PeerResponseRepliedAttached(corr_id) ==
    /\ phase = "Attached"
    /\ (corr_id \in DOMAIN inbound_peer_requests)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapRemove(inbound_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests >>


PeerResponseRepliedRunning(corr_id) ==
    /\ phase = "Running"
    /\ (corr_id \in DOMAIN inbound_peer_requests)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapRemove(inbound_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests >>


PeerResponseRepliedRetired(corr_id) ==
    /\ phase = "Retired"
    /\ (corr_id \in DOMAIN inbound_peer_requests)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapRemove(inbound_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests >>


PeerResponseRepliedStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ (corr_id \in DOMAIN inbound_peer_requests)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapRemove(inbound_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests >>


BeginLiveTopologyReconfigureIdle(authority_epoch) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Reconfiguring"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


BeginLiveTopologyReconfigureAttached(authority_epoch) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Reconfiguring"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


BeginLiveTopologyReconfigureRunning(authority_epoch) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Reconfiguring"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


BeginLiveTopologyReconfigureRetired(authority_epoch) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Reconfiguring"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


BeginLiveTopologyReconfigureStopped(authority_epoch) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Reconfiguring"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ApplyLiveTopologyIdentityIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (live_topology_phase = "Detached")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostIdentityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ApplyLiveTopologyIdentityAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (live_topology_phase = "Detached")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostIdentityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ApplyLiveTopologyIdentityRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (live_topology_phase = "Detached")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostIdentityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ApplyLiveTopologyIdentityRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (live_topology_phase = "Detached")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostIdentityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ApplyLiveTopologyIdentityStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (live_topology_phase = "Detached")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostIdentityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ApplyLiveTopologyVisibilityIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostIdentityApplied")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostVisibilityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ApplyLiveTopologyVisibilityAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostIdentityApplied")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostVisibilityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ApplyLiveTopologyVisibilityRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostIdentityApplied")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostVisibilityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ApplyLiveTopologyVisibilityRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostIdentityApplied")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostVisibilityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


ApplyLiveTopologyVisibilityStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostIdentityApplied")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostVisibilityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


CompleteLiveTopologyIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostVisibilityApplied")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


CompleteLiveTopologyAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostVisibilityApplied")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


CompleteLiveTopologyRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostVisibilityApplied")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


CompleteLiveTopologyRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostVisibilityApplied")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


CompleteLiveTopologyStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostVisibilityApplied")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AbortLiveTopologyBeforeDetachIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AbortLiveTopologyBeforeDetachAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AbortLiveTopologyBeforeDetachRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AbortLiveTopologyBeforeDetachRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


AbortLiveTopologyBeforeDetachStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, mcp_server_states, pending_peer_requests, inbound_peer_requests >>


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
    \/ \E input_id \in InputIdValues : \E request_immediate_processing \in BOOLEAN : \E interrupt_yielding \in BOOLEAN : \E wake_if_idle \in BOOLEAN : \E run_id \in RunIdValues : AcceptWithCompletionIdleQueued(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id)
    \/ \E input_id \in InputIdValues : \E request_immediate_processing \in BOOLEAN : \E interrupt_yielding \in BOOLEAN : \E wake_if_idle \in BOOLEAN : \E run_id \in RunIdValues : AcceptWithCompletionIdleImmediate(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id)
    \/ \E input_id \in InputIdValues : \E request_immediate_processing \in BOOLEAN : \E interrupt_yielding \in BOOLEAN : \E wake_if_idle \in BOOLEAN : \E run_id \in RunIdValues : AcceptWithCompletionAttachedImmediate(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id)
    \/ \E input_id \in InputIdValues : \E request_immediate_processing \in BOOLEAN : \E interrupt_yielding \in BOOLEAN : \E wake_if_idle \in BOOLEAN : \E run_id \in RunIdValues : AcceptWithCompletionAttachedQueued(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id)
    \/ \E input_id \in InputIdValues : \E request_immediate_processing \in BOOLEAN : \E interrupt_yielding \in BOOLEAN : \E wake_if_idle \in BOOLEAN : \E run_id \in RunIdValues : AcceptWithCompletionRunningQueuedPassive(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id)
    \/ \E input_id \in InputIdValues : \E request_immediate_processing \in BOOLEAN : \E interrupt_yielding \in BOOLEAN : \E wake_if_idle \in BOOLEAN : \E run_id \in RunIdValues : AcceptWithCompletionRunningQueuedWakeIfIdle(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id)
    \/ \E input_id \in InputIdValues : \E request_immediate_processing \in BOOLEAN : \E interrupt_yielding \in BOOLEAN : \E wake_if_idle \in BOOLEAN : \E run_id \in RunIdValues : AcceptWithCompletionRunningInterruptYielding(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id)
    \/ \E input_id \in InputIdValues : \E request_immediate_processing \in BOOLEAN : \E interrupt_yielding \in BOOLEAN : \E wake_if_idle \in BOOLEAN : \E run_id \in RunIdValues : AcceptWithCompletionRunningImmediate(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id)
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
    \/ \E authority_epoch \in 0..2 : \E next_binding_state \in RealtimeBindingStateValues : PublishRealtimeSignalIdle(authority_epoch, next_binding_state)
    \/ \E authority_epoch \in 0..2 : \E next_binding_state \in RealtimeBindingStateValues : PublishRealtimeSignalAttached(authority_epoch, next_binding_state)
    \/ \E authority_epoch \in 0..2 : \E next_binding_state \in RealtimeBindingStateValues : PublishRealtimeSignalRunning(authority_epoch, next_binding_state)
    \/ \E authority_epoch \in 0..2 : \E next_binding_state \in RealtimeBindingStateValues : PublishRealtimeSignalRetired(authority_epoch, next_binding_state)
    \/ \E authority_epoch \in 0..2 : \E next_binding_state \in RealtimeBindingStateValues : PublishRealtimeSignalStopped(authority_epoch, next_binding_state)
    \/ \E server_id \in McpServerIdValues : McpServerConnectPendingIdle(server_id)
    \/ \E server_id \in McpServerIdValues : McpServerConnectPendingAttached(server_id)
    \/ \E server_id \in McpServerIdValues : McpServerConnectPendingRunning(server_id)
    \/ \E server_id \in McpServerIdValues : McpServerConnectPendingRetired(server_id)
    \/ \E server_id \in McpServerIdValues : McpServerConnectPendingStopped(server_id)
    \/ \E server_id \in McpServerIdValues : McpServerConnectedIdle(server_id)
    \/ \E server_id \in McpServerIdValues : McpServerConnectedAttached(server_id)
    \/ \E server_id \in McpServerIdValues : McpServerConnectedRunning(server_id)
    \/ \E server_id \in McpServerIdValues : McpServerConnectedRetired(server_id)
    \/ \E server_id \in McpServerIdValues : McpServerConnectedStopped(server_id)
    \/ \E server_id \in McpServerIdValues : \E error \in StringValues : McpServerFailedIdle(server_id, error)
    \/ \E server_id \in McpServerIdValues : \E error \in StringValues : McpServerFailedAttached(server_id, error)
    \/ \E server_id \in McpServerIdValues : \E error \in StringValues : McpServerFailedRunning(server_id, error)
    \/ \E server_id \in McpServerIdValues : \E error \in StringValues : McpServerFailedRetired(server_id, error)
    \/ \E server_id \in McpServerIdValues : \E error \in StringValues : McpServerFailedStopped(server_id, error)
    \/ \E server_id \in McpServerIdValues : McpServerDisconnectedIdle(server_id)
    \/ \E server_id \in McpServerIdValues : McpServerDisconnectedAttached(server_id)
    \/ \E server_id \in McpServerIdValues : McpServerDisconnectedRunning(server_id)
    \/ \E server_id \in McpServerIdValues : McpServerDisconnectedRetired(server_id)
    \/ \E server_id \in McpServerIdValues : McpServerDisconnectedStopped(server_id)
    \/ \E server_id \in McpServerIdValues : McpServerReloadIdle(server_id)
    \/ \E server_id \in McpServerIdValues : McpServerReloadAttached(server_id)
    \/ \E server_id \in McpServerIdValues : McpServerReloadRunning(server_id)
    \/ \E server_id \in McpServerIdValues : McpServerReloadRetired(server_id)
    \/ \E server_id \in McpServerIdValues : McpServerReloadStopped(server_id)
    \/ \E corr_id \in PeerCorrelationIdValues : \E to \in StringValues : PeerRequestSentIdle(corr_id, to)
    \/ \E corr_id \in PeerCorrelationIdValues : \E to \in StringValues : PeerRequestSentAttached(corr_id, to)
    \/ \E corr_id \in PeerCorrelationIdValues : \E to \in StringValues : PeerRequestSentRunning(corr_id, to)
    \/ \E corr_id \in PeerCorrelationIdValues : \E to \in StringValues : PeerRequestSentRetired(corr_id, to)
    \/ \E corr_id \in PeerCorrelationIdValues : \E to \in StringValues : PeerRequestSentStopped(corr_id, to)
    \/ \E corr_id \in PeerCorrelationIdValues : PeerResponseProgressArrivedIdle(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : PeerResponseProgressArrivedAttached(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : PeerResponseProgressArrivedRunning(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : PeerResponseProgressArrivedRetired(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : PeerResponseProgressArrivedStopped(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : \E disposition \in PeerTerminalDispositionValues : PeerResponseTerminalArrivedCompletedIdle(corr_id, disposition)
    \/ \E corr_id \in PeerCorrelationIdValues : \E disposition \in PeerTerminalDispositionValues : PeerResponseTerminalArrivedCompletedAttached(corr_id, disposition)
    \/ \E corr_id \in PeerCorrelationIdValues : \E disposition \in PeerTerminalDispositionValues : PeerResponseTerminalArrivedCompletedRunning(corr_id, disposition)
    \/ \E corr_id \in PeerCorrelationIdValues : \E disposition \in PeerTerminalDispositionValues : PeerResponseTerminalArrivedCompletedRetired(corr_id, disposition)
    \/ \E corr_id \in PeerCorrelationIdValues : \E disposition \in PeerTerminalDispositionValues : PeerResponseTerminalArrivedCompletedStopped(corr_id, disposition)
    \/ \E corr_id \in PeerCorrelationIdValues : \E disposition \in PeerTerminalDispositionValues : PeerResponseTerminalArrivedFailedIdle(corr_id, disposition)
    \/ \E corr_id \in PeerCorrelationIdValues : \E disposition \in PeerTerminalDispositionValues : PeerResponseTerminalArrivedFailedAttached(corr_id, disposition)
    \/ \E corr_id \in PeerCorrelationIdValues : \E disposition \in PeerTerminalDispositionValues : PeerResponseTerminalArrivedFailedRunning(corr_id, disposition)
    \/ \E corr_id \in PeerCorrelationIdValues : \E disposition \in PeerTerminalDispositionValues : PeerResponseTerminalArrivedFailedRetired(corr_id, disposition)
    \/ \E corr_id \in PeerCorrelationIdValues : \E disposition \in PeerTerminalDispositionValues : PeerResponseTerminalArrivedFailedStopped(corr_id, disposition)
    \/ \E corr_id \in PeerCorrelationIdValues : PeerRequestTimedOutIdle(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : PeerRequestTimedOutAttached(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : PeerRequestTimedOutRunning(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : PeerRequestTimedOutRetired(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : PeerRequestTimedOutStopped(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : PeerRequestReceivedIdle(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : PeerRequestReceivedAttached(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : PeerRequestReceivedRunning(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : PeerRequestReceivedRetired(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : PeerRequestReceivedStopped(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : PeerResponseRepliedIdle(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : PeerResponseRepliedAttached(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : PeerResponseRepliedRunning(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : PeerResponseRepliedRetired(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : PeerResponseRepliedStopped(corr_id)
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

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(silent_intent_overrides) <= 1 /\ Cardinality(DOMAIN mcp_server_states) <= 1 /\ Cardinality(DOMAIN pending_peer_requests) <= 1 /\ Cardinality(DOMAIN inbound_peer_requests) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(silent_intent_overrides) <= 2 /\ Cardinality(DOMAIN mcp_server_states) <= 2 /\ Cardinality(DOMAIN pending_peer_requests) <= 2 /\ Cardinality(DOMAIN inbound_peer_requests) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []fence_requires_bound_runtime
THEOREM Spec => []running_has_current_run
THEOREM Spec => []current_run_only_while_running_or_retired
THEOREM Spec => []realtime_binding_epoch_consistency

=============================================================================
