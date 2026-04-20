---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MeerkatMachine.

CONSTANTS AgentRuntimeIdValues, BooleanValues, CommsRuntimeIdValues, FenceTokenValues, GenerationValues, InboundPeerRequestStateValues, InputIdValues, LiveTopologyPhaseValues, McpServerIdValues, McpServerStateValues, MobIdValues, NatValues, OutboundPeerRequestStateValues, PeerCorrelationIdValues, PeerIngressOwnerKindValues, PeerTerminalDispositionValues, RealtimeBindingStateValues, RealtimeProductTurnPhaseValues, RunIdValues, SessionIdValues, SessionLlmCapabilitySurfaceStatusValues, SessionLlmCapabilitySurfaceValues, SessionLlmIdentityValues, SessionToolVisibilityDeltaValues, SessionToolVisibilityStateValues, SetOfPeerCorrelationIdValues, SetOfStringValues, StringValues, ToolFilterValues, ToolVisibilityWitnessValues, WorkIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionAgentRuntimeIdValues == {None} \cup {Some(x) : x \in AgentRuntimeIdValues}
OptionCommsRuntimeIdValues == {None} \cup {Some(x) : x \in CommsRuntimeIdValues}
OptionFenceTokenValues == {None} \cup {Some(x) : x \in FenceTokenValues}
OptionMobIdValues == {None} \cup {Some(x) : x \in MobIdValues}
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

VARIABLES phase, model_step_count, session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id

vars == << phase, model_step_count, session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>

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
    /\ last_session_context_updated_at_ms = 0
    /\ reserved_interaction_streams = {}
    /\ attached_interaction_streams = {}
    /\ realtime_product_turn_phase = "Idle"
    /\ peer_ingress_owner_kind = "Unattached"
    /\ peer_ingress_comms_runtime_id = None
    /\ peer_ingress_mob_id = None

TerminalStutter ==
    /\ phase = "Destroyed"
    /\ UNCHANGED vars

Initialize ==
    /\ phase = "Initializing"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


RegisterSessionIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


RegisterSessionAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


RegisterSessionRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


RegisterSessionRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


RegisterSessionStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ReconfigureSessionLlmIdentityAttached(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (active_runtime_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ReconfigureSessionLlmIdentityRunning(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (active_runtime_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


StagePersistentFilterIdle(filter, witnesses) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


StagePersistentFilterAttached(filter, witnesses) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


StagePersistentFilterRunning(filter, witnesses) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


StagePersistentFilterRetired(filter, witnesses) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


StagePersistentFilterStopped(filter, witnesses) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


RequestDeferredToolsIdle(names, witnesses) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


RequestDeferredToolsAttached(names, witnesses) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


RequestDeferredToolsRunning(names, witnesses) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


RequestDeferredToolsRetired(names, witnesses) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


RequestDeferredToolsStopped(names, witnesses) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PrepareBindingsInitializing(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Initializing"
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PrepareBindingsIdle(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Idle"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PrepareBindingsAttached(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PrepareBindingsRunning(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PrepareBindingsRetired(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PrepareBindingsStopped(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


SetPeerIngressContextIdle(keep_alive) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


SetPeerIngressContextAttached(keep_alive) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


SetPeerIngressContextRunning(keep_alive) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


SetPeerIngressContextRetired(keep_alive) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


SetPeerIngressContextStopped(keep_alive) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


NotifyDrainExitedIdle(reason) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


NotifyDrainExitedAttached(reason) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


NotifyDrainExitedRunning(reason) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


NotifyDrainExitedRetired(reason) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


NotifyDrainExitedStopped(reason) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InterruptCurrentRunAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InterruptCurrentRun ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


CancelAfterBoundaryAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


CancelAfterBoundary ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


BoundaryAppliedPublish(revision) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PublishCommittedVisibleSetIdle(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PublishCommittedVisibleSetAttached(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PublishCommittedVisibleSetRunning(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PublishCommittedVisibleSetRetired(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PublishCommittedVisibleSetStopped(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


RetireRequestedFromIdle ==
    /\ phase = "Idle" \/ phase = "Attached" \/ phase = "Running"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


Reset ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Attached" \/ phase = "Retired"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ active_fence_token' = None
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


StopRuntimeExecutorUnbound ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Retired"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


StopRuntimeExecutorAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


StopRuntimeExecutorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


Destroy ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Attached" \/ phase = "Running" \/ phase = "Retired" \/ phase = "Stopped"
    /\ (active_runtime_id # None)
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


EnsureSessionWithExecutorIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


EnsureSessionWithExecutorAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


EnsureSessionWithExecutorRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


EnsureSessionWithExecutorRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


EnsureSessionWithExecutorStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


SetSilentIntentsIdle(arg_session_id, intents) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


SetSilentIntentsAttached(arg_session_id, intents) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


SetSilentIntentsRunning(arg_session_id, intents) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


SetSilentIntentsRetired(arg_session_id, intents) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


SetSilentIntentsStopped(arg_session_id, intents) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AbortIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AbortAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AbortRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AbortRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AbortStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


WaitIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


WaitAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


WaitRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


WaitRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


WaitStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AbortAllIdle ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AbortAllAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AbortAllRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AbortAllRetired ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AbortAllStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


EnsureDrainRunningAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


EnsureDrainRunningRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


IngestIdle(runtime_id, work_id, origin) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


IngestAttached(runtime_id, work_id, origin) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


IngestRunning(runtime_id, work_id, origin) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PublishEventIdle(kind) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PublishEventAttached(kind) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PublishEventRunning(kind) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PublishEventRetired(kind) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PublishEventStopped(kind) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AcceptWithCompletionIdleQueued(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AcceptWithCompletionIdleImmediate(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (request_immediate_processing = TRUE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AcceptWithCompletionAttachedImmediate(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (request_immediate_processing = TRUE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("attached")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AcceptWithCompletionAttachedQueued(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AcceptWithCompletionRunningQueuedPassive(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ (wake_if_idle = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AcceptWithCompletionRunningQueuedWakeIfIdle(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ (wake_if_idle = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AcceptWithCompletionRunningInterruptYielding(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AcceptWithCompletionRunningImmediate(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = TRUE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AcceptWithoutWakeIdle(input_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AcceptWithoutWakeAttached(input_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AcceptWithoutWakeRunning(input_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ClassifyExternalEnvelopeAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ClassifyExternalEnvelopeRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ClassifyPlainEventAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ClassifyPlainEventRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PrepareIdle(arg_session_id, run_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("idle")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PrepareAttached(arg_session_id, run_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("attached")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


DrainQueuedRunRetired(run_id) ==
    /\ phase = "Retired"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("retired")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


StartConversationRunAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


StartImmediateAppendAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


StartImmediateContextAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


CommitRunningToIdle(input_id, run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("idle"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


CommitRunningToAttached(input_id, run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("attached"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


CommitRunningToRetired(input_id, run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("retired"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


FailRunningToIdle(run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("idle"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


FailRunningToAttached(run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("attached"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


FailRunningToRetired(run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("retired"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


StageAddAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


StageAddRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


StageRemoveAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


StageRemoveRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


StageReloadAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


StageReloadRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ApplySurfaceBoundaryAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ApplySurfaceBoundaryRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PendingSucceededAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PendingSucceededRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PendingFailedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PendingFailedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


CallStartedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


CallStartedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


CallFinishedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


CallFinishedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


FinalizeRemovalCleanAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


FinalizeRemovalCleanRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


FinalizeRemovalForcedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


FinalizeRemovalForcedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


SnapshotAlignedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


SnapshotAlignedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ShutdownSurfaceAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ShutdownSurfaceRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


RecycleFromIdleOrRetired ==
    /\ phase = "Idle" \/ phase = "Retired"
    /\ (active_runtime_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ active_fence_token' = None
    /\ current_run_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


RecycleFromAttached ==
    /\ phase = "Attached"
    /\ (active_runtime_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_fence_token' = None
    /\ current_run_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProjectRealtimeIntentIdle(present) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_intent_present' = present
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProjectRealtimeIntentAttached(present) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_intent_present' = present
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProjectRealtimeIntentRunning(present) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_intent_present' = present
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProjectRealtimeIntentRetired(present) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_intent_present' = present
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProjectRealtimeIntentStopped(present) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_intent_present' = present
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


DetachRealtimeBindingIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


DetachRealtimeBindingAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


DetachRealtimeBindingRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


DetachRealtimeBindingRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


DetachRealtimeBindingStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


RequireRealtimeReattachIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


RequireRealtimeReattachAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


RequireRealtimeReattachRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


RequireRealtimeReattachRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


RequireRealtimeReattachStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PublishRealtimeSignalIdle(authority_epoch, next_binding_state) ==
    /\ phase = "Idle"
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ ((next_binding_state = "BindingNotReady") \/ (next_binding_state = "BindingReady") \/ (next_binding_state = "ReplacementPending"))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = next_binding_state
    /\ realtime_reattach_required' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_authority_epoch, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PublishRealtimeSignalAttached(authority_epoch, next_binding_state) ==
    /\ phase = "Attached"
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ ((next_binding_state = "BindingNotReady") \/ (next_binding_state = "BindingReady") \/ (next_binding_state = "ReplacementPending"))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = next_binding_state
    /\ realtime_reattach_required' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_authority_epoch, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PublishRealtimeSignalRunning(authority_epoch, next_binding_state) ==
    /\ phase = "Running"
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ ((next_binding_state = "BindingNotReady") \/ (next_binding_state = "BindingReady") \/ (next_binding_state = "ReplacementPending"))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = next_binding_state
    /\ realtime_reattach_required' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_authority_epoch, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PublishRealtimeSignalRetired(authority_epoch, next_binding_state) ==
    /\ phase = "Retired"
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ ((next_binding_state = "BindingNotReady") \/ (next_binding_state = "BindingReady") \/ (next_binding_state = "ReplacementPending"))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = next_binding_state
    /\ realtime_reattach_required' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_authority_epoch, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PublishRealtimeSignalStopped(authority_epoch, next_binding_state) ==
    /\ phase = "Stopped"
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ ((next_binding_state = "BindingNotReady") \/ (next_binding_state = "BindingReady") \/ (next_binding_state = "ReplacementPending"))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = next_binding_state
    /\ realtime_reattach_required' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_authority_epoch, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerConnectPendingIdle(server_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerConnectPendingAttached(server_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerConnectPendingRunning(server_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerConnectPendingRetired(server_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerConnectPendingStopped(server_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerConnectedIdle(server_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Connected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerConnectedAttached(server_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Connected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerConnectedRunning(server_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Connected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerConnectedRetired(server_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Connected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerConnectedStopped(server_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Connected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerFailedIdle(server_id, error) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Failed")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerFailedAttached(server_id, error) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Failed")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerFailedRunning(server_id, error) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Failed")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerFailedRetired(server_id, error) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Failed")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerFailedStopped(server_id, error) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Failed")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerDisconnectedIdle(server_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Disconnected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerDisconnectedAttached(server_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Disconnected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerDisconnectedRunning(server_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Disconnected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerDisconnectedRetired(server_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Disconnected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerDisconnectedStopped(server_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Disconnected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerReloadIdle(server_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerReloadAttached(server_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerReloadRunning(server_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerReloadRetired(server_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


McpServerReloadStopped(server_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerRequestSentIdle(corr_id, to) ==
    /\ phase = "Idle"
    /\ ~((corr_id \in DOMAIN pending_peer_requests))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "Sent")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerRequestSentAttached(corr_id, to) ==
    /\ phase = "Attached"
    /\ ~((corr_id \in DOMAIN pending_peer_requests))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "Sent")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerRequestSentRunning(corr_id, to) ==
    /\ phase = "Running"
    /\ ~((corr_id \in DOMAIN pending_peer_requests))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "Sent")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerRequestSentRetired(corr_id, to) ==
    /\ phase = "Retired"
    /\ ~((corr_id \in DOMAIN pending_peer_requests))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "Sent")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerRequestSentStopped(corr_id, to) ==
    /\ phase = "Stopped"
    /\ ~((corr_id \in DOMAIN pending_peer_requests))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "Sent")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerResponseProgressArrivedIdle(corr_id) ==
    /\ phase = "Idle"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "AcceptedProgress")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerResponseProgressArrivedAttached(corr_id) ==
    /\ phase = "Attached"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "AcceptedProgress")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerResponseProgressArrivedRunning(corr_id) ==
    /\ phase = "Running"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "AcceptedProgress")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerResponseProgressArrivedRetired(corr_id) ==
    /\ phase = "Retired"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "AcceptedProgress")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerResponseProgressArrivedStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "AcceptedProgress")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerResponseTerminalArrivedCompletedIdle(corr_id, disposition) ==
    /\ phase = "Idle"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Completed")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerResponseTerminalArrivedCompletedAttached(corr_id, disposition) ==
    /\ phase = "Attached"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Completed")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerResponseTerminalArrivedCompletedRunning(corr_id, disposition) ==
    /\ phase = "Running"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Completed")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerResponseTerminalArrivedCompletedRetired(corr_id, disposition) ==
    /\ phase = "Retired"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Completed")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerResponseTerminalArrivedCompletedStopped(corr_id, disposition) ==
    /\ phase = "Stopped"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Completed")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerResponseTerminalArrivedFailedIdle(corr_id, disposition) ==
    /\ phase = "Idle"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Failed")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerResponseTerminalArrivedFailedAttached(corr_id, disposition) ==
    /\ phase = "Attached"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Failed")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerResponseTerminalArrivedFailedRunning(corr_id, disposition) ==
    /\ phase = "Running"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Failed")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerResponseTerminalArrivedFailedRetired(corr_id, disposition) ==
    /\ phase = "Retired"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Failed")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerResponseTerminalArrivedFailedStopped(corr_id, disposition) ==
    /\ phase = "Stopped"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Failed")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerRequestTimedOutIdle(corr_id) ==
    /\ phase = "Idle"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerRequestTimedOutAttached(corr_id) ==
    /\ phase = "Attached"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerRequestTimedOutRunning(corr_id) ==
    /\ phase = "Running"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerRequestTimedOutRetired(corr_id) ==
    /\ phase = "Retired"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerRequestTimedOutStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerRequestReceivedIdle(corr_id) ==
    /\ phase = "Idle"
    /\ ~((corr_id \in DOMAIN inbound_peer_requests))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapSet(inbound_peer_requests, corr_id, "Received")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerRequestReceivedAttached(corr_id) ==
    /\ phase = "Attached"
    /\ ~((corr_id \in DOMAIN inbound_peer_requests))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapSet(inbound_peer_requests, corr_id, "Received")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerRequestReceivedRunning(corr_id) ==
    /\ phase = "Running"
    /\ ~((corr_id \in DOMAIN inbound_peer_requests))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapSet(inbound_peer_requests, corr_id, "Received")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerRequestReceivedRetired(corr_id) ==
    /\ phase = "Retired"
    /\ ~((corr_id \in DOMAIN inbound_peer_requests))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapSet(inbound_peer_requests, corr_id, "Received")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerRequestReceivedStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ ~((corr_id \in DOMAIN inbound_peer_requests))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapSet(inbound_peer_requests, corr_id, "Received")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerResponseRepliedIdle(corr_id) ==
    /\ phase = "Idle"
    /\ (corr_id \in DOMAIN inbound_peer_requests)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapRemove(inbound_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerResponseRepliedAttached(corr_id) ==
    /\ phase = "Attached"
    /\ (corr_id \in DOMAIN inbound_peer_requests)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapRemove(inbound_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerResponseRepliedRunning(corr_id) ==
    /\ phase = "Running"
    /\ (corr_id \in DOMAIN inbound_peer_requests)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapRemove(inbound_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerResponseRepliedRetired(corr_id) ==
    /\ phase = "Retired"
    /\ (corr_id \in DOMAIN inbound_peer_requests)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapRemove(inbound_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


PeerResponseRepliedStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ (corr_id \in DOMAIN inbound_peer_requests)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapRemove(inbound_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AdvanceSessionContextIdle(updated_at_ms) ==
    /\ phase = "Idle"
    /\ (updated_at_ms > last_session_context_updated_at_ms)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ last_session_context_updated_at_ms' = updated_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AdvanceSessionContextAttached(updated_at_ms) ==
    /\ phase = "Attached"
    /\ (updated_at_ms > last_session_context_updated_at_ms)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ last_session_context_updated_at_ms' = updated_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AdvanceSessionContextRunning(updated_at_ms) ==
    /\ phase = "Running"
    /\ (updated_at_ms > last_session_context_updated_at_ms)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ last_session_context_updated_at_ms' = updated_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AdvanceSessionContextRetired(updated_at_ms) ==
    /\ phase = "Retired"
    /\ (updated_at_ms > last_session_context_updated_at_ms)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ last_session_context_updated_at_ms' = updated_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AdvanceSessionContextStopped(updated_at_ms) ==
    /\ phase = "Stopped"
    /\ (updated_at_ms > last_session_context_updated_at_ms)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ last_session_context_updated_at_ms' = updated_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamReservedIdle(corr_id) ==
    /\ phase = "Idle"
    /\ ~((corr_id \in reserved_interaction_streams))
    /\ ~((corr_id \in attached_interaction_streams))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \cup {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamReservedAttached(corr_id) ==
    /\ phase = "Attached"
    /\ ~((corr_id \in reserved_interaction_streams))
    /\ ~((corr_id \in attached_interaction_streams))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \cup {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamReservedRunning(corr_id) ==
    /\ phase = "Running"
    /\ ~((corr_id \in reserved_interaction_streams))
    /\ ~((corr_id \in attached_interaction_streams))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \cup {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamReservedRetired(corr_id) ==
    /\ phase = "Retired"
    /\ ~((corr_id \in reserved_interaction_streams))
    /\ ~((corr_id \in attached_interaction_streams))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \cup {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamReservedStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ ~((corr_id \in reserved_interaction_streams))
    /\ ~((corr_id \in attached_interaction_streams))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \cup {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamAttachedIdle(corr_id) ==
    /\ phase = "Idle"
    /\ (corr_id \in reserved_interaction_streams)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \ {corr_id})
    /\ attached_interaction_streams' = (attached_interaction_streams \cup {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamAttachedAttached(corr_id) ==
    /\ phase = "Attached"
    /\ (corr_id \in reserved_interaction_streams)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \ {corr_id})
    /\ attached_interaction_streams' = (attached_interaction_streams \cup {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamAttachedRunning(corr_id) ==
    /\ phase = "Running"
    /\ (corr_id \in reserved_interaction_streams)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \ {corr_id})
    /\ attached_interaction_streams' = (attached_interaction_streams \cup {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamAttachedRetired(corr_id) ==
    /\ phase = "Retired"
    /\ (corr_id \in reserved_interaction_streams)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \ {corr_id})
    /\ attached_interaction_streams' = (attached_interaction_streams \cup {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamAttachedStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ (corr_id \in reserved_interaction_streams)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \ {corr_id})
    /\ attached_interaction_streams' = (attached_interaction_streams \cup {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamCompletedIdle(corr_id) ==
    /\ phase = "Idle"
    /\ (corr_id \in attached_interaction_streams)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ attached_interaction_streams' = (attached_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamCompletedAttached(corr_id) ==
    /\ phase = "Attached"
    /\ (corr_id \in attached_interaction_streams)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ attached_interaction_streams' = (attached_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamCompletedRunning(corr_id) ==
    /\ phase = "Running"
    /\ (corr_id \in attached_interaction_streams)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ attached_interaction_streams' = (attached_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamCompletedRetired(corr_id) ==
    /\ phase = "Retired"
    /\ (corr_id \in attached_interaction_streams)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ attached_interaction_streams' = (attached_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamCompletedStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ (corr_id \in attached_interaction_streams)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ attached_interaction_streams' = (attached_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamExpiredIdle(corr_id) ==
    /\ phase = "Idle"
    /\ (corr_id \in reserved_interaction_streams)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamExpiredAttached(corr_id) ==
    /\ phase = "Attached"
    /\ (corr_id \in reserved_interaction_streams)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamExpiredRunning(corr_id) ==
    /\ phase = "Running"
    /\ (corr_id \in reserved_interaction_streams)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamExpiredRetired(corr_id) ==
    /\ phase = "Retired"
    /\ (corr_id \in reserved_interaction_streams)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamExpiredStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ (corr_id \in reserved_interaction_streams)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamClosedEarlyIdle(corr_id) ==
    /\ phase = "Idle"
    /\ (corr_id \in attached_interaction_streams)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ attached_interaction_streams' = (attached_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamClosedEarlyAttached(corr_id) ==
    /\ phase = "Attached"
    /\ (corr_id \in attached_interaction_streams)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ attached_interaction_streams' = (attached_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamClosedEarlyRunning(corr_id) ==
    /\ phase = "Running"
    /\ (corr_id \in attached_interaction_streams)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ attached_interaction_streams' = (attached_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamClosedEarlyRetired(corr_id) ==
    /\ phase = "Retired"
    /\ (corr_id \in attached_interaction_streams)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ attached_interaction_streams' = (attached_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


InteractionStreamClosedEarlyStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ (corr_id \in attached_interaction_streams)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ attached_interaction_streams' = (attached_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnInFlightInitializing ==
    /\ phase = "Initializing"
    /\ (realtime_product_turn_phase = "Idle")
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnInFlightIdle ==
    /\ phase = "Idle"
    /\ (realtime_product_turn_phase = "Idle")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnInFlightAttached ==
    /\ phase = "Attached"
    /\ (realtime_product_turn_phase = "Idle")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnInFlightRunning ==
    /\ phase = "Running"
    /\ (realtime_product_turn_phase = "Idle")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnInFlightRetired ==
    /\ phase = "Retired"
    /\ (realtime_product_turn_phase = "Idle")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnInFlightStopped ==
    /\ phase = "Stopped"
    /\ (realtime_product_turn_phase = "Idle")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnCommittedFromAwaitingInitializing ==
    /\ phase = "Initializing"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnCommittedFromAwaitingIdle ==
    /\ phase = "Idle"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnCommittedFromAwaitingAttached ==
    /\ phase = "Attached"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnCommittedFromAwaitingRunning ==
    /\ phase = "Running"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnCommittedFromAwaitingRetired ==
    /\ phase = "Retired"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnCommittedFromAwaitingStopped ==
    /\ phase = "Stopped"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnCommittedFromOutputInitializing ==
    /\ phase = "Initializing"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnCommittedFromOutputIdle ==
    /\ phase = "Idle"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnCommittedFromOutputAttached ==
    /\ phase = "Attached"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnCommittedFromOutputRunning ==
    /\ phase = "Running"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnCommittedFromOutputRetired ==
    /\ phase = "Retired"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnCommittedFromOutputStopped ==
    /\ phase = "Stopped"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductOutputStartedFromAwaitingInitializing ==
    /\ phase = "Initializing"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "OutputStarted"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductOutputStartedFromAwaitingIdle ==
    /\ phase = "Idle"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "OutputStarted"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductOutputStartedFromAwaitingAttached ==
    /\ phase = "Attached"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "OutputStarted"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductOutputStartedFromAwaitingRunning ==
    /\ phase = "Running"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "OutputStarted"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductOutputStartedFromAwaitingRetired ==
    /\ phase = "Retired"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "OutputStarted"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductOutputStartedFromAwaitingStopped ==
    /\ phase = "Stopped"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "OutputStarted"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductOutputStartedFromCommittedInitializing ==
    /\ phase = "Initializing"
    /\ (realtime_product_turn_phase = "Committed")
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductOutputStartedFromCommittedIdle ==
    /\ phase = "Idle"
    /\ (realtime_product_turn_phase = "Committed")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductOutputStartedFromCommittedAttached ==
    /\ phase = "Attached"
    /\ (realtime_product_turn_phase = "Committed")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductOutputStartedFromCommittedRunning ==
    /\ phase = "Running"
    /\ (realtime_product_turn_phase = "Committed")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductOutputStartedFromCommittedRetired ==
    /\ phase = "Retired"
    /\ (realtime_product_turn_phase = "Committed")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductOutputStartedFromCommittedStopped ==
    /\ phase = "Stopped"
    /\ (realtime_product_turn_phase = "Committed")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnInterruptedFromPreemptibleInitializing ==
    /\ phase = "Initializing"
    /\ (realtime_product_turn_phase = "Preemptible")
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnInterruptedFromPreemptibleIdle ==
    /\ phase = "Idle"
    /\ (realtime_product_turn_phase = "Preemptible")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnInterruptedFromPreemptibleAttached ==
    /\ phase = "Attached"
    /\ (realtime_product_turn_phase = "Preemptible")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnInterruptedFromPreemptibleRunning ==
    /\ phase = "Running"
    /\ (realtime_product_turn_phase = "Preemptible")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnInterruptedFromPreemptibleRetired ==
    /\ phase = "Retired"
    /\ (realtime_product_turn_phase = "Preemptible")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnInterruptedFromPreemptibleStopped ==
    /\ phase = "Stopped"
    /\ (realtime_product_turn_phase = "Preemptible")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnInterruptedFromOutputInitializing ==
    /\ phase = "Initializing"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnInterruptedFromOutputIdle ==
    /\ phase = "Idle"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnInterruptedFromOutputAttached ==
    /\ phase = "Attached"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnInterruptedFromOutputRunning ==
    /\ phase = "Running"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnInterruptedFromOutputRetired ==
    /\ phase = "Retired"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnInterruptedFromOutputStopped ==
    /\ phase = "Stopped"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnTerminalInitializing ==
    /\ phase = "Initializing"
    /\ (realtime_product_turn_phase # "Idle")
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnTerminalIdle ==
    /\ phase = "Idle"
    /\ (realtime_product_turn_phase # "Idle")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnTerminalAttached ==
    /\ phase = "Attached"
    /\ (realtime_product_turn_phase # "Idle")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnTerminalRunning ==
    /\ phase = "Running"
    /\ (realtime_product_turn_phase # "Idle")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnTerminalRetired ==
    /\ phase = "Retired"
    /\ (realtime_product_turn_phase # "Idle")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ProductTurnTerminalStopped ==
    /\ phase = "Stopped"
    /\ (realtime_product_turn_phase # "Idle")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


BeginLiveTopologyReconfigureIdle(authority_epoch) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Reconfiguring"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


BeginLiveTopologyReconfigureAttached(authority_epoch) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Reconfiguring"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


BeginLiveTopologyReconfigureRunning(authority_epoch) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Reconfiguring"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


BeginLiveTopologyReconfigureRetired(authority_epoch) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Reconfiguring"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


BeginLiveTopologyReconfigureStopped(authority_epoch) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Reconfiguring"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ApplyLiveTopologyIdentityIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (live_topology_phase = "Detached")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostIdentityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ApplyLiveTopologyIdentityAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (live_topology_phase = "Detached")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostIdentityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ApplyLiveTopologyIdentityRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (live_topology_phase = "Detached")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostIdentityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ApplyLiveTopologyIdentityRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (live_topology_phase = "Detached")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostIdentityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ApplyLiveTopologyIdentityStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (live_topology_phase = "Detached")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostIdentityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ApplyLiveTopologyVisibilityIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostIdentityApplied")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostVisibilityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ApplyLiveTopologyVisibilityAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostIdentityApplied")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostVisibilityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ApplyLiveTopologyVisibilityRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostIdentityApplied")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostVisibilityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ApplyLiveTopologyVisibilityRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostIdentityApplied")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostVisibilityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


ApplyLiveTopologyVisibilityStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostIdentityApplied")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostVisibilityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


CompleteLiveTopologyIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostVisibilityApplied")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


CompleteLiveTopologyAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostVisibilityApplied")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


CompleteLiveTopologyRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostVisibilityApplied")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


CompleteLiveTopologyRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostVisibilityApplied")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


CompleteLiveTopologyStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostVisibilityApplied")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AbortLiveTopologyBeforeDetachIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AbortLiveTopologyBeforeDetachAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AbortLiveTopologyBeforeDetachRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AbortLiveTopologyBeforeDetachRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AbortLiveTopologyBeforeDetachStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id >>


AttachSessionIngressIdle(comms_runtime_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (peer_ingress_owner_kind = "Unattached")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "SessionOwned"
    /\ peer_ingress_comms_runtime_id' = Some(comms_runtime_id)
    /\ peer_ingress_mob_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase >>


AttachSessionIngressAttached(comms_runtime_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (peer_ingress_owner_kind = "Unattached")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "SessionOwned"
    /\ peer_ingress_comms_runtime_id' = Some(comms_runtime_id)
    /\ peer_ingress_mob_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase >>


AttachSessionIngressRunning(comms_runtime_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (peer_ingress_owner_kind = "Unattached")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "SessionOwned"
    /\ peer_ingress_comms_runtime_id' = Some(comms_runtime_id)
    /\ peer_ingress_mob_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase >>


AttachSessionIngressRetired(comms_runtime_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (peer_ingress_owner_kind = "Unattached")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "SessionOwned"
    /\ peer_ingress_comms_runtime_id' = Some(comms_runtime_id)
    /\ peer_ingress_mob_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase >>


AttachSessionIngressStopped(comms_runtime_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (peer_ingress_owner_kind = "Unattached")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "SessionOwned"
    /\ peer_ingress_comms_runtime_id' = Some(comms_runtime_id)
    /\ peer_ingress_mob_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase >>


AttachMobIngressIdle(comms_runtime_id, mob_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ ((peer_ingress_owner_kind = "Unattached") \/ (peer_ingress_owner_kind = "SessionOwned"))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "MobOwned"
    /\ peer_ingress_comms_runtime_id' = Some(comms_runtime_id)
    /\ peer_ingress_mob_id' = Some(mob_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase >>


AttachMobIngressAttached(comms_runtime_id, mob_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ ((peer_ingress_owner_kind = "Unattached") \/ (peer_ingress_owner_kind = "SessionOwned"))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "MobOwned"
    /\ peer_ingress_comms_runtime_id' = Some(comms_runtime_id)
    /\ peer_ingress_mob_id' = Some(mob_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase >>


AttachMobIngressRunning(comms_runtime_id, mob_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ ((peer_ingress_owner_kind = "Unattached") \/ (peer_ingress_owner_kind = "SessionOwned"))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "MobOwned"
    /\ peer_ingress_comms_runtime_id' = Some(comms_runtime_id)
    /\ peer_ingress_mob_id' = Some(mob_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase >>


AttachMobIngressRetired(comms_runtime_id, mob_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ ((peer_ingress_owner_kind = "Unattached") \/ (peer_ingress_owner_kind = "SessionOwned"))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "MobOwned"
    /\ peer_ingress_comms_runtime_id' = Some(comms_runtime_id)
    /\ peer_ingress_mob_id' = Some(mob_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase >>


AttachMobIngressStopped(comms_runtime_id, mob_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ ((peer_ingress_owner_kind = "Unattached") \/ (peer_ingress_owner_kind = "SessionOwned"))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "MobOwned"
    /\ peer_ingress_comms_runtime_id' = Some(comms_runtime_id)
    /\ peer_ingress_mob_id' = Some(mob_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase >>


DetachIngressIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (peer_ingress_owner_kind # "Unattached")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "Unattached"
    /\ peer_ingress_comms_runtime_id' = None
    /\ peer_ingress_mob_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase >>


DetachIngressAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (peer_ingress_owner_kind # "Unattached")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "Unattached"
    /\ peer_ingress_comms_runtime_id' = None
    /\ peer_ingress_mob_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase >>


DetachIngressRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (peer_ingress_owner_kind # "Unattached")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "Unattached"
    /\ peer_ingress_comms_runtime_id' = None
    /\ peer_ingress_mob_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase >>


DetachIngressRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (peer_ingress_owner_kind # "Unattached")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "Unattached"
    /\ peer_ingress_comms_runtime_id' = None
    /\ peer_ingress_mob_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase >>


DetachIngressStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (peer_ingress_owner_kind # "Unattached")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "Unattached"
    /\ peer_ingress_comms_runtime_id' = None
    /\ peer_ingress_mob_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase >>


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
    \/ \E updated_at_ms \in 0..2 : AdvanceSessionContextIdle(updated_at_ms)
    \/ \E updated_at_ms \in 0..2 : AdvanceSessionContextAttached(updated_at_ms)
    \/ \E updated_at_ms \in 0..2 : AdvanceSessionContextRunning(updated_at_ms)
    \/ \E updated_at_ms \in 0..2 : AdvanceSessionContextRetired(updated_at_ms)
    \/ \E updated_at_ms \in 0..2 : AdvanceSessionContextStopped(updated_at_ms)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamReservedIdle(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamReservedAttached(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamReservedRunning(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamReservedRetired(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamReservedStopped(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamAttachedIdle(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamAttachedAttached(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamAttachedRunning(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamAttachedRetired(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamAttachedStopped(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamCompletedIdle(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamCompletedAttached(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamCompletedRunning(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamCompletedRetired(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamCompletedStopped(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamExpiredIdle(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamExpiredAttached(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamExpiredRunning(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamExpiredRetired(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamExpiredStopped(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamClosedEarlyIdle(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamClosedEarlyAttached(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamClosedEarlyRunning(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamClosedEarlyRetired(corr_id)
    \/ \E corr_id \in PeerCorrelationIdValues : InteractionStreamClosedEarlyStopped(corr_id)
    \/ ProductTurnInFlightInitializing
    \/ ProductTurnInFlightIdle
    \/ ProductTurnInFlightAttached
    \/ ProductTurnInFlightRunning
    \/ ProductTurnInFlightRetired
    \/ ProductTurnInFlightStopped
    \/ ProductTurnCommittedFromAwaitingInitializing
    \/ ProductTurnCommittedFromAwaitingIdle
    \/ ProductTurnCommittedFromAwaitingAttached
    \/ ProductTurnCommittedFromAwaitingRunning
    \/ ProductTurnCommittedFromAwaitingRetired
    \/ ProductTurnCommittedFromAwaitingStopped
    \/ ProductTurnCommittedFromOutputInitializing
    \/ ProductTurnCommittedFromOutputIdle
    \/ ProductTurnCommittedFromOutputAttached
    \/ ProductTurnCommittedFromOutputRunning
    \/ ProductTurnCommittedFromOutputRetired
    \/ ProductTurnCommittedFromOutputStopped
    \/ ProductOutputStartedFromAwaitingInitializing
    \/ ProductOutputStartedFromAwaitingIdle
    \/ ProductOutputStartedFromAwaitingAttached
    \/ ProductOutputStartedFromAwaitingRunning
    \/ ProductOutputStartedFromAwaitingRetired
    \/ ProductOutputStartedFromAwaitingStopped
    \/ ProductOutputStartedFromCommittedInitializing
    \/ ProductOutputStartedFromCommittedIdle
    \/ ProductOutputStartedFromCommittedAttached
    \/ ProductOutputStartedFromCommittedRunning
    \/ ProductOutputStartedFromCommittedRetired
    \/ ProductOutputStartedFromCommittedStopped
    \/ ProductTurnInterruptedFromPreemptibleInitializing
    \/ ProductTurnInterruptedFromPreemptibleIdle
    \/ ProductTurnInterruptedFromPreemptibleAttached
    \/ ProductTurnInterruptedFromPreemptibleRunning
    \/ ProductTurnInterruptedFromPreemptibleRetired
    \/ ProductTurnInterruptedFromPreemptibleStopped
    \/ ProductTurnInterruptedFromOutputInitializing
    \/ ProductTurnInterruptedFromOutputIdle
    \/ ProductTurnInterruptedFromOutputAttached
    \/ ProductTurnInterruptedFromOutputRunning
    \/ ProductTurnInterruptedFromOutputRetired
    \/ ProductTurnInterruptedFromOutputStopped
    \/ ProductTurnTerminalInitializing
    \/ ProductTurnTerminalIdle
    \/ ProductTurnTerminalAttached
    \/ ProductTurnTerminalRunning
    \/ ProductTurnTerminalRetired
    \/ ProductTurnTerminalStopped
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
    \/ \E comms_runtime_id \in CommsRuntimeIdValues : AttachSessionIngressIdle(comms_runtime_id)
    \/ \E comms_runtime_id \in CommsRuntimeIdValues : AttachSessionIngressAttached(comms_runtime_id)
    \/ \E comms_runtime_id \in CommsRuntimeIdValues : AttachSessionIngressRunning(comms_runtime_id)
    \/ \E comms_runtime_id \in CommsRuntimeIdValues : AttachSessionIngressRetired(comms_runtime_id)
    \/ \E comms_runtime_id \in CommsRuntimeIdValues : AttachSessionIngressStopped(comms_runtime_id)
    \/ \E comms_runtime_id \in CommsRuntimeIdValues : \E mob_id \in MobIdValues : AttachMobIngressIdle(comms_runtime_id, mob_id)
    \/ \E comms_runtime_id \in CommsRuntimeIdValues : \E mob_id \in MobIdValues : AttachMobIngressAttached(comms_runtime_id, mob_id)
    \/ \E comms_runtime_id \in CommsRuntimeIdValues : \E mob_id \in MobIdValues : AttachMobIngressRunning(comms_runtime_id, mob_id)
    \/ \E comms_runtime_id \in CommsRuntimeIdValues : \E mob_id \in MobIdValues : AttachMobIngressRetired(comms_runtime_id, mob_id)
    \/ \E comms_runtime_id \in CommsRuntimeIdValues : \E mob_id \in MobIdValues : AttachMobIngressStopped(comms_runtime_id, mob_id)
    \/ DetachIngressIdle
    \/ DetachIngressAttached
    \/ DetachIngressRunning
    \/ DetachIngressRetired
    \/ DetachIngressStopped
    \/ TerminalStutter

fence_requires_bound_runtime == ((active_fence_token = None) \/ (active_runtime_id # None))
running_has_current_run == ((phase # "Running") \/ (current_run_id # None))
current_run_only_while_running_or_retired == ((current_run_id = None) \/ (phase = "Running") \/ (phase = "Retired"))
realtime_binding_epoch_consistency == ((realtime_binding_state = "Unbound") = (realtime_binding_authority_epoch = None))
peer_ingress_owner_consistency == (((peer_ingress_owner_kind = "Unattached") /\ (peer_ingress_comms_runtime_id = None) /\ (peer_ingress_mob_id = None)) \/ ((peer_ingress_owner_kind = "SessionOwned") /\ (peer_ingress_comms_runtime_id # None) /\ (peer_ingress_mob_id = None)) \/ ((peer_ingress_owner_kind = "MobOwned") /\ (peer_ingress_comms_runtime_id # None) /\ (peer_ingress_mob_id # None)))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(silent_intent_overrides) <= 1 /\ Cardinality(DOMAIN mcp_server_states) <= 1 /\ Cardinality(DOMAIN pending_peer_requests) <= 1 /\ Cardinality(DOMAIN inbound_peer_requests) <= 1 /\ Cardinality(reserved_interaction_streams) <= 1 /\ Cardinality(attached_interaction_streams) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(silent_intent_overrides) <= 2 /\ Cardinality(DOMAIN mcp_server_states) <= 2 /\ Cardinality(DOMAIN pending_peer_requests) <= 2 /\ Cardinality(DOMAIN inbound_peer_requests) <= 2 /\ Cardinality(reserved_interaction_streams) <= 2 /\ Cardinality(attached_interaction_streams) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []fence_requires_bound_runtime
THEOREM Spec => []running_has_current_run
THEOREM Spec => []current_run_only_while_running_or_retired
THEOREM Spec => []realtime_binding_epoch_consistency
THEOREM Spec => []peer_ingress_owner_consistency

=============================================================================
