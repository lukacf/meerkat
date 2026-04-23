---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for MeerkatMachine.

CONSTANTS AgentRuntimeIdValues, BooleanValues, CommsRuntimeIdValues, FenceTokenValues, GenerationValues, InboundPeerRequestStateValues, InputIdValues, LiveTopologyPhaseValues, McpServerIdValues, McpServerStateValues, MobIdValues, NatValues, OperationIdValues, OutboundPeerRequestStateValues, PeerCorrelationIdValues, PeerEndpointValues, PeerIngressOwnerKindValues, PeerTerminalDispositionValues, RealtimeBindingStateValues, RealtimeProductTurnPhaseValues, RealtimeProjectionFreshnessValues, RealtimeReconnectPolicyValues, RunIdValues, SessionIdValues, SessionLlmCapabilitySurfaceStatusValues, SessionLlmCapabilitySurfaceValues, SessionLlmIdentityValues, SessionToolVisibilityDeltaValues, SessionToolVisibilityStateValues, SetOfOperationIdValues, SetOfPeerCorrelationIdValues, SetOfPeerEndpointValues, SetOfStringValues, StringValues, SupervisorBindingKindValues, ToolFilterValues, ToolVisibilityWitnessValues, WorkIdValues, WorkOriginValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionAgentRuntimeIdValues == {None} \cup {Some(x) : x \in AgentRuntimeIdValues}
OptionCommsRuntimeIdValues == {None} \cup {Some(x) : x \in CommsRuntimeIdValues}
OptionFenceTokenValues == {None} \cup {Some(x) : x \in FenceTokenValues}
OptionMobIdValues == {None} \cup {Some(x) : x \in MobIdValues}
OptionPeerEndpointValues == {None} \cup {Some(x) : x \in PeerEndpointValues}
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

VARIABLES phase, model_step_count, session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch

vars == << phase, model_step_count, session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>

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
    /\ realtime_reconnect_attempt_count = 0
    /\ realtime_reconnect_next_retry_at_ms = None
    /\ realtime_reconnect_deadline_at_ms = None
    /\ live_topology_phase = "Idle"
    /\ mcp_server_states = [x \in {} |-> None]
    /\ pending_peer_requests = [x \in {} |-> None]
    /\ inbound_peer_requests = [x \in {} |-> None]
    /\ last_session_context_updated_at_ms = 0
    /\ reserved_interaction_streams = {}
    /\ attached_interaction_streams = {}
    /\ realtime_product_turn_phase = "Idle"
    /\ realtime_projection_freshness = "Clean"
    /\ realtime_projection_frontier_ms = 0
    /\ realtime_reconnect_policy = "CleanExit"
    /\ peer_ingress_owner_kind = "Unattached"
    /\ peer_ingress_comms_runtime_id = None
    /\ peer_ingress_mob_id = None
    /\ supervisor_binding_kind = "Unbound"
    /\ supervisor_bound_name = None
    /\ supervisor_bound_peer_id = None
    /\ supervisor_bound_address = None
    /\ supervisor_bound_epoch = None
    /\ local_endpoint = None
    /\ direct_peer_endpoints = {}
    /\ mob_overlay_peer_endpoints = {}
    /\ peer_projection_epoch = 0
    /\ mob_overlay_epoch = 0

TerminalStutter ==
    /\ phase = "Destroyed"
    /\ UNCHANGED vars

Initialize ==
    /\ phase = "Initializing"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RegisterSessionIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RegisterSessionAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RegisterSessionRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RegisterSessionRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RegisterSessionStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ session_id' = Some(arg_session_id)
    /\ UNCHANGED << active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ReconfigureSessionLlmIdentityAttached(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (active_runtime_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ReconfigureSessionLlmIdentityRunning(previous_identity, previous_visibility_state, previous_capability_surface, previous_capability_surface_status, target_identity, target_capability_surface, next_visibility_state, next_capability_base_filter, next_active_visibility_revision, tool_visibility_delta) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (active_runtime_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


StagePersistentFilterIdle(filter, witnesses) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


StagePersistentFilterAttached(filter, witnesses) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


StagePersistentFilterRunning(filter, witnesses) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


StagePersistentFilterRetired(filter, witnesses) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


StagePersistentFilterStopped(filter, witnesses) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RequestDeferredToolsIdle(names, witnesses) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RequestDeferredToolsAttached(names, witnesses) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RequestDeferredToolsRunning(names, witnesses) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RequestDeferredToolsRetired(names, witnesses) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RequestDeferredToolsStopped(names, witnesses) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PrepareBindingsInitializing(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Initializing"
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PrepareBindingsIdle(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Idle"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PrepareBindingsAttached(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PrepareBindingsRunning(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PrepareBindingsRetired(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PrepareBindingsStopped(agent_runtime_id, fence_token, generation) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ active_runtime_id' = Some(agent_runtime_id)
    /\ active_fence_token' = Some(fence_token)
    /\ UNCHANGED << session_id, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SetPeerIngressContextIdle(keep_alive) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SetPeerIngressContextAttached(keep_alive) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SetPeerIngressContextRunning(keep_alive) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SetPeerIngressContextRetired(keep_alive) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SetPeerIngressContextStopped(keep_alive) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


NotifyDrainExitedIdle(reason) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


NotifyDrainExitedAttached(reason) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


NotifyDrainExitedRunning(reason) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


NotifyDrainExitedRetired(reason) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


NotifyDrainExitedStopped(reason) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InterruptCurrentRunAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InterruptCurrentRun ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


CancelAfterBoundaryAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


CancelAfterBoundary ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


BoundaryAppliedPublish(revision) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PublishCommittedVisibleSetIdle(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PublishCommittedVisibleSetAttached(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PublishCommittedVisibleSetRunning(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PublishCommittedVisibleSetRetired(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PublishCommittedVisibleSetStopped(active_filter, staged_filter, active_requested_deferred_names, staged_requested_deferred_names, active_visibility_revision, staged_visibility_revision) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (active_visibility_revision >= staged_visibility_revision)
    /\ ((active_visibility_revision # staged_visibility_revision) \/ ((active_filter = staged_filter) /\ (active_requested_deferred_names = staged_requested_deferred_names)))
    /\ (\A requested_name \in active_requested_deferred_names : (requested_name \in staged_requested_deferred_names))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RetireRequestedFromIdle ==
    /\ phase = "Idle" \/ phase = "Attached" \/ phase = "Running"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


Reset ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Attached" \/ phase = "Retired"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ active_fence_token' = None
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


StopRuntimeExecutorUnbound ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Retired"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


StopRuntimeExecutorAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


StopRuntimeExecutorRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


Destroy ==
    /\ phase = "Initializing" \/ phase = "Idle" \/ phase = "Attached" \/ phase = "Running" \/ phase = "Retired" \/ phase = "Stopped"
    /\ (active_runtime_id # None)
    /\ phase' = "Destroyed"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ silent_intent_overrides' = {}
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


EnsureSessionWithExecutorIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


EnsureSessionWithExecutorAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


EnsureSessionWithExecutorRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


EnsureSessionWithExecutorRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


EnsureSessionWithExecutorStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SetSilentIntentsIdle(arg_session_id, intents) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SetSilentIntentsAttached(arg_session_id, intents) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SetSilentIntentsRunning(arg_session_id, intents) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SetSilentIntentsRetired(arg_session_id, intents) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ silent_intent_overrides' = intents
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SetSilentIntentsStopped(arg_session_id, intents) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AbortIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AbortAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AbortRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AbortRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AbortStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


WaitIdle(arg_session_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


WaitAttached(arg_session_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


WaitRunning(arg_session_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


WaitRetired(arg_session_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


WaitStopped(arg_session_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AbortAllIdle ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AbortAllAttached ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AbortAllRunning ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AbortAllRetired ==
    /\ phase = "Retired"
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AbortAllStopped ==
    /\ phase = "Stopped"
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


EnsureDrainRunningAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


EnsureDrainRunningRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


IngestIdle(runtime_id, work_id, origin) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


IngestAttached(runtime_id, work_id, origin) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


IngestRunning(runtime_id, work_id, origin) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PublishEventIdle(kind) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PublishEventAttached(kind) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PublishEventRunning(kind) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PublishEventRetired(kind) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PublishEventStopped(kind) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AcceptWithCompletionIdleQueued(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AcceptWithCompletionIdleImmediate(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (request_immediate_processing = TRUE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AcceptWithCompletionAttachedImmediate(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (request_immediate_processing = TRUE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("attached")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AcceptWithCompletionAttachedQueued(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AcceptWithCompletionRunningQueuedPassive(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ (wake_if_idle = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AcceptWithCompletionRunningQueuedWakeIfIdle(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = FALSE)
    /\ (wake_if_idle = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AcceptWithCompletionRunningInterruptYielding(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = FALSE)
    /\ (interrupt_yielding = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AcceptWithCompletionRunningImmediate(input_id, request_immediate_processing, interrupt_yielding, wake_if_idle, run_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (request_immediate_processing = TRUE)
    /\ (interrupt_yielding = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AcceptWithoutWakeIdle(input_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AcceptWithoutWakeAttached(input_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AcceptWithoutWakeRunning(input_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyExternalEnvelopeAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyExternalEnvelopeRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyPlainEventAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyPlainEventRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PrepareIdle(arg_session_id, run_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("idle")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PrepareAttached(arg_session_id, run_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("attached")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


DrainQueuedRunRetired(run_id) ==
    /\ phase = "Retired"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = Some(run_id)
    /\ pre_run_phase' = Some("retired")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


StartConversationRunAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


StartImmediateAppendAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


StartImmediateContextAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


CommitRunningToIdle(input_id, run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("idle"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


CommitRunningToAttached(input_id, run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("attached"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


CommitRunningToRetired(input_id, run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("retired"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


FailRunningToIdle(run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("idle"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


FailRunningToAttached(run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("attached"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


FailRunningToRetired(run_id) ==
    /\ phase = "Running"
    /\ (pre_run_phase = Some("retired"))
    /\ (current_run_id = Some(run_id))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ current_run_id' = None
    /\ pre_run_phase' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


StageAddAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


StageAddRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


StageRemoveAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


StageRemoveRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


StageReloadAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


StageReloadRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ApplySurfaceBoundaryAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ApplySurfaceBoundaryRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PendingSucceededAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PendingSucceededRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PendingFailedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PendingFailedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


CallStartedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


CallStartedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


CallFinishedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


CallFinishedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


FinalizeRemovalCleanAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


FinalizeRemovalCleanRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


FinalizeRemovalForcedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


FinalizeRemovalForcedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SnapshotAlignedAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SnapshotAlignedRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ShutdownSurfaceAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ShutdownSurfaceRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RecycleFromIdleOrRetired ==
    /\ phase = "Idle" \/ phase = "Retired"
    /\ (active_runtime_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ active_fence_token' = None
    /\ current_run_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RecycleFromAttached ==
    /\ phase = "Attached"
    /\ (active_runtime_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ active_fence_token' = None
    /\ current_run_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProjectRealtimeIntentIdle(present) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_intent_present' = present
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProjectRealtimeIntentAttached(present) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_intent_present' = present
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProjectRealtimeIntentRunning(present) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_intent_present' = present
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProjectRealtimeIntentRetired(present) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_intent_present' = present
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProjectRealtimeIntentStopped(present) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_intent_present' = present
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


DetachRealtimeBindingIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


DetachRealtimeBindingAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


DetachRealtimeBindingRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


DetachRealtimeBindingRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


DetachRealtimeBindingStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = FALSE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RequireRealtimeReattachIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RequireRealtimeReattachAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RequireRealtimeReattachRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RequireRealtimeReattachRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RequireRealtimeReattachStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = "Unbound"
    /\ realtime_binding_authority_epoch' = None
    /\ realtime_reattach_required' = TRUE
    /\ realtime_next_authority_epoch' = (realtime_next_authority_epoch + 1)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PublishRealtimeSignalIdle(authority_epoch, next_binding_state) ==
    /\ phase = "Idle"
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ ((next_binding_state = "BindingNotReady") \/ (next_binding_state = "BindingReady") \/ (next_binding_state = "ReplacementPending"))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = next_binding_state
    /\ realtime_reattach_required' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_authority_epoch, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PublishRealtimeSignalAttached(authority_epoch, next_binding_state) ==
    /\ phase = "Attached"
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ ((next_binding_state = "BindingNotReady") \/ (next_binding_state = "BindingReady") \/ (next_binding_state = "ReplacementPending"))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = next_binding_state
    /\ realtime_reattach_required' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_authority_epoch, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PublishRealtimeSignalRunning(authority_epoch, next_binding_state) ==
    /\ phase = "Running"
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ ((next_binding_state = "BindingNotReady") \/ (next_binding_state = "BindingReady") \/ (next_binding_state = "ReplacementPending"))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = next_binding_state
    /\ realtime_reattach_required' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_authority_epoch, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PublishRealtimeSignalRetired(authority_epoch, next_binding_state) ==
    /\ phase = "Retired"
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ ((next_binding_state = "BindingNotReady") \/ (next_binding_state = "BindingReady") \/ (next_binding_state = "ReplacementPending"))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = next_binding_state
    /\ realtime_reattach_required' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_authority_epoch, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PublishRealtimeSignalStopped(authority_epoch, next_binding_state) ==
    /\ phase = "Stopped"
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ ((next_binding_state = "BindingNotReady") \/ (next_binding_state = "BindingReady") \/ (next_binding_state = "ReplacementPending"))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_binding_state' = next_binding_state
    /\ realtime_reattach_required' = FALSE
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_authority_epoch, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProjectRealtimeReconnectProgressIdle(attempt_count, next_retry_at_ms, deadline_at_ms) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_attempt_count' = attempt_count
    /\ realtime_reconnect_next_retry_at_ms' = next_retry_at_ms
    /\ realtime_reconnect_deadline_at_ms' = deadline_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProjectRealtimeReconnectProgressAttached(attempt_count, next_retry_at_ms, deadline_at_ms) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_attempt_count' = attempt_count
    /\ realtime_reconnect_next_retry_at_ms' = next_retry_at_ms
    /\ realtime_reconnect_deadline_at_ms' = deadline_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProjectRealtimeReconnectProgressRunning(attempt_count, next_retry_at_ms, deadline_at_ms) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_attempt_count' = attempt_count
    /\ realtime_reconnect_next_retry_at_ms' = next_retry_at_ms
    /\ realtime_reconnect_deadline_at_ms' = deadline_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProjectRealtimeReconnectProgressRetired(attempt_count, next_retry_at_ms, deadline_at_ms) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_attempt_count' = attempt_count
    /\ realtime_reconnect_next_retry_at_ms' = next_retry_at_ms
    /\ realtime_reconnect_deadline_at_ms' = deadline_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProjectRealtimeReconnectProgressStopped(attempt_count, next_retry_at_ms, deadline_at_ms) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_attempt_count' = attempt_count
    /\ realtime_reconnect_next_retry_at_ms' = next_retry_at_ms
    /\ realtime_reconnect_deadline_at_ms' = deadline_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClearRealtimeReconnectProgressIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_attempt_count' = 0
    /\ realtime_reconnect_next_retry_at_ms' = None
    /\ realtime_reconnect_deadline_at_ms' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClearRealtimeReconnectProgressAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_attempt_count' = 0
    /\ realtime_reconnect_next_retry_at_ms' = None
    /\ realtime_reconnect_deadline_at_ms' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClearRealtimeReconnectProgressRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_attempt_count' = 0
    /\ realtime_reconnect_next_retry_at_ms' = None
    /\ realtime_reconnect_deadline_at_ms' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClearRealtimeReconnectProgressRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_attempt_count' = 0
    /\ realtime_reconnect_next_retry_at_ms' = None
    /\ realtime_reconnect_deadline_at_ms' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClearRealtimeReconnectProgressStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_attempt_count' = 0
    /\ realtime_reconnect_next_retry_at_ms' = None
    /\ realtime_reconnect_deadline_at_ms' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerConnectPendingIdle(server_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerConnectPendingAttached(server_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerConnectPendingRunning(server_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerConnectPendingRetired(server_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerConnectPendingStopped(server_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerConnectedIdle(server_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Connected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerConnectedAttached(server_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Connected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerConnectedRunning(server_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Connected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerConnectedRetired(server_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Connected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerConnectedStopped(server_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Connected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerFailedIdle(server_id, error) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Failed")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerFailedAttached(server_id, error) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Failed")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerFailedRunning(server_id, error) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Failed")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerFailedRetired(server_id, error) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Failed")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerFailedStopped(server_id, error) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Failed")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerDisconnectedIdle(server_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Disconnected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerDisconnectedAttached(server_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Disconnected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerDisconnectedRunning(server_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Disconnected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerDisconnectedRetired(server_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Disconnected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerDisconnectedStopped(server_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "Disconnected")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerReloadIdle(server_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerReloadAttached(server_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerReloadRunning(server_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerReloadRetired(server_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


McpServerReloadStopped(server_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ mcp_server_states' = MapSet(mcp_server_states, server_id, "PendingConnect")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerRequestSentIdle(corr_id, to) ==
    /\ phase = "Idle"
    /\ ~((corr_id \in DOMAIN pending_peer_requests))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "Sent")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerRequestSentAttached(corr_id, to) ==
    /\ phase = "Attached"
    /\ ~((corr_id \in DOMAIN pending_peer_requests))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "Sent")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerRequestSentRunning(corr_id, to) ==
    /\ phase = "Running"
    /\ ~((corr_id \in DOMAIN pending_peer_requests))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "Sent")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerRequestSentRetired(corr_id, to) ==
    /\ phase = "Retired"
    /\ ~((corr_id \in DOMAIN pending_peer_requests))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "Sent")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerRequestSentStopped(corr_id, to) ==
    /\ phase = "Stopped"
    /\ ~((corr_id \in DOMAIN pending_peer_requests))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "Sent")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerResponseProgressArrivedIdle(corr_id) ==
    /\ phase = "Idle"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "AcceptedProgress")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerResponseProgressArrivedAttached(corr_id) ==
    /\ phase = "Attached"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "AcceptedProgress")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerResponseProgressArrivedRunning(corr_id) ==
    /\ phase = "Running"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "AcceptedProgress")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerResponseProgressArrivedRetired(corr_id) ==
    /\ phase = "Retired"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "AcceptedProgress")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerResponseProgressArrivedStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapSet(pending_peer_requests, corr_id, "AcceptedProgress")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerResponseTerminalArrivedCompletedIdle(corr_id, disposition) ==
    /\ phase = "Idle"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Completed")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerResponseTerminalArrivedCompletedAttached(corr_id, disposition) ==
    /\ phase = "Attached"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Completed")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerResponseTerminalArrivedCompletedRunning(corr_id, disposition) ==
    /\ phase = "Running"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Completed")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerResponseTerminalArrivedCompletedRetired(corr_id, disposition) ==
    /\ phase = "Retired"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Completed")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerResponseTerminalArrivedCompletedStopped(corr_id, disposition) ==
    /\ phase = "Stopped"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Completed")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerResponseTerminalArrivedFailedIdle(corr_id, disposition) ==
    /\ phase = "Idle"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Failed")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerResponseTerminalArrivedFailedAttached(corr_id, disposition) ==
    /\ phase = "Attached"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Failed")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerResponseTerminalArrivedFailedRunning(corr_id, disposition) ==
    /\ phase = "Running"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Failed")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerResponseTerminalArrivedFailedRetired(corr_id, disposition) ==
    /\ phase = "Retired"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Failed")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerResponseTerminalArrivedFailedStopped(corr_id, disposition) ==
    /\ phase = "Stopped"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ (disposition = "Failed")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerRequestTimedOutIdle(corr_id) ==
    /\ phase = "Idle"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerRequestTimedOutAttached(corr_id) ==
    /\ phase = "Attached"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerRequestTimedOutRunning(corr_id) ==
    /\ phase = "Running"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerRequestTimedOutRetired(corr_id) ==
    /\ phase = "Retired"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerRequestTimedOutStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ (corr_id \in DOMAIN pending_peer_requests)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ pending_peer_requests' = MapRemove(pending_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerRequestReceivedIdle(corr_id) ==
    /\ phase = "Idle"
    /\ ~((corr_id \in DOMAIN inbound_peer_requests))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapSet(inbound_peer_requests, corr_id, "Received")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerRequestReceivedAttached(corr_id) ==
    /\ phase = "Attached"
    /\ ~((corr_id \in DOMAIN inbound_peer_requests))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapSet(inbound_peer_requests, corr_id, "Received")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerRequestReceivedRunning(corr_id) ==
    /\ phase = "Running"
    /\ ~((corr_id \in DOMAIN inbound_peer_requests))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapSet(inbound_peer_requests, corr_id, "Received")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerRequestReceivedRetired(corr_id) ==
    /\ phase = "Retired"
    /\ ~((corr_id \in DOMAIN inbound_peer_requests))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapSet(inbound_peer_requests, corr_id, "Received")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerRequestReceivedStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ ~((corr_id \in DOMAIN inbound_peer_requests))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapSet(inbound_peer_requests, corr_id, "Received")
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerResponseRepliedIdle(corr_id) ==
    /\ phase = "Idle"
    /\ (corr_id \in DOMAIN inbound_peer_requests)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapRemove(inbound_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerResponseRepliedAttached(corr_id) ==
    /\ phase = "Attached"
    /\ (corr_id \in DOMAIN inbound_peer_requests)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapRemove(inbound_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerResponseRepliedRunning(corr_id) ==
    /\ phase = "Running"
    /\ (corr_id \in DOMAIN inbound_peer_requests)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapRemove(inbound_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerResponseRepliedRetired(corr_id) ==
    /\ phase = "Retired"
    /\ (corr_id \in DOMAIN inbound_peer_requests)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapRemove(inbound_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PeerResponseRepliedStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ (corr_id \in DOMAIN inbound_peer_requests)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ inbound_peer_requests' = MapRemove(inbound_peer_requests, corr_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AdvanceSessionContextIdle(updated_at_ms) ==
    /\ phase = "Idle"
    /\ (updated_at_ms > last_session_context_updated_at_ms)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ last_session_context_updated_at_ms' = updated_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AdvanceSessionContextAttached(updated_at_ms) ==
    /\ phase = "Attached"
    /\ (updated_at_ms > last_session_context_updated_at_ms)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ last_session_context_updated_at_ms' = updated_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AdvanceSessionContextRunning(updated_at_ms) ==
    /\ phase = "Running"
    /\ (updated_at_ms > last_session_context_updated_at_ms)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ last_session_context_updated_at_ms' = updated_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AdvanceSessionContextRetired(updated_at_ms) ==
    /\ phase = "Retired"
    /\ (updated_at_ms > last_session_context_updated_at_ms)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ last_session_context_updated_at_ms' = updated_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AdvanceSessionContextStopped(updated_at_ms) ==
    /\ phase = "Stopped"
    /\ (updated_at_ms > last_session_context_updated_at_ms)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ last_session_context_updated_at_ms' = updated_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamReservedIdle(corr_id) ==
    /\ phase = "Idle"
    /\ ~((corr_id \in reserved_interaction_streams))
    /\ ~((corr_id \in attached_interaction_streams))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \cup {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamReservedAttached(corr_id) ==
    /\ phase = "Attached"
    /\ ~((corr_id \in reserved_interaction_streams))
    /\ ~((corr_id \in attached_interaction_streams))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \cup {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamReservedRunning(corr_id) ==
    /\ phase = "Running"
    /\ ~((corr_id \in reserved_interaction_streams))
    /\ ~((corr_id \in attached_interaction_streams))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \cup {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamReservedRetired(corr_id) ==
    /\ phase = "Retired"
    /\ ~((corr_id \in reserved_interaction_streams))
    /\ ~((corr_id \in attached_interaction_streams))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \cup {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamReservedStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ ~((corr_id \in reserved_interaction_streams))
    /\ ~((corr_id \in attached_interaction_streams))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \cup {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamAttachedIdle(corr_id) ==
    /\ phase = "Idle"
    /\ (corr_id \in reserved_interaction_streams)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \ {corr_id})
    /\ attached_interaction_streams' = (attached_interaction_streams \cup {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamAttachedAttached(corr_id) ==
    /\ phase = "Attached"
    /\ (corr_id \in reserved_interaction_streams)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \ {corr_id})
    /\ attached_interaction_streams' = (attached_interaction_streams \cup {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamAttachedRunning(corr_id) ==
    /\ phase = "Running"
    /\ (corr_id \in reserved_interaction_streams)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \ {corr_id})
    /\ attached_interaction_streams' = (attached_interaction_streams \cup {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamAttachedRetired(corr_id) ==
    /\ phase = "Retired"
    /\ (corr_id \in reserved_interaction_streams)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \ {corr_id})
    /\ attached_interaction_streams' = (attached_interaction_streams \cup {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamAttachedStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ (corr_id \in reserved_interaction_streams)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \ {corr_id})
    /\ attached_interaction_streams' = (attached_interaction_streams \cup {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamCompletedIdle(corr_id) ==
    /\ phase = "Idle"
    /\ (corr_id \in attached_interaction_streams)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ attached_interaction_streams' = (attached_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamCompletedAttached(corr_id) ==
    /\ phase = "Attached"
    /\ (corr_id \in attached_interaction_streams)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ attached_interaction_streams' = (attached_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamCompletedRunning(corr_id) ==
    /\ phase = "Running"
    /\ (corr_id \in attached_interaction_streams)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ attached_interaction_streams' = (attached_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamCompletedRetired(corr_id) ==
    /\ phase = "Retired"
    /\ (corr_id \in attached_interaction_streams)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ attached_interaction_streams' = (attached_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamCompletedStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ (corr_id \in attached_interaction_streams)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ attached_interaction_streams' = (attached_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamExpiredIdle(corr_id) ==
    /\ phase = "Idle"
    /\ (corr_id \in reserved_interaction_streams)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamExpiredAttached(corr_id) ==
    /\ phase = "Attached"
    /\ (corr_id \in reserved_interaction_streams)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamExpiredRunning(corr_id) ==
    /\ phase = "Running"
    /\ (corr_id \in reserved_interaction_streams)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamExpiredRetired(corr_id) ==
    /\ phase = "Retired"
    /\ (corr_id \in reserved_interaction_streams)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamExpiredStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ (corr_id \in reserved_interaction_streams)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ reserved_interaction_streams' = (reserved_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamClosedEarlyIdle(corr_id) ==
    /\ phase = "Idle"
    /\ (corr_id \in attached_interaction_streams)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ attached_interaction_streams' = (attached_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamClosedEarlyAttached(corr_id) ==
    /\ phase = "Attached"
    /\ (corr_id \in attached_interaction_streams)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ attached_interaction_streams' = (attached_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamClosedEarlyRunning(corr_id) ==
    /\ phase = "Running"
    /\ (corr_id \in attached_interaction_streams)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ attached_interaction_streams' = (attached_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamClosedEarlyRetired(corr_id) ==
    /\ phase = "Retired"
    /\ (corr_id \in attached_interaction_streams)
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ attached_interaction_streams' = (attached_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


InteractionStreamClosedEarlyStopped(corr_id) ==
    /\ phase = "Stopped"
    /\ (corr_id \in attached_interaction_streams)
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ attached_interaction_streams' = (attached_interaction_streams \ {corr_id})
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnInFlightInitializing ==
    /\ phase = "Initializing"
    /\ (realtime_product_turn_phase = "Idle")
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnInFlightIdle ==
    /\ phase = "Idle"
    /\ (realtime_product_turn_phase = "Idle")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnInFlightAttached ==
    /\ phase = "Attached"
    /\ (realtime_product_turn_phase = "Idle")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnInFlightRunning ==
    /\ phase = "Running"
    /\ (realtime_product_turn_phase = "Idle")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnInFlightRetired ==
    /\ phase = "Retired"
    /\ (realtime_product_turn_phase = "Idle")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnInFlightStopped ==
    /\ phase = "Stopped"
    /\ (realtime_product_turn_phase = "Idle")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnCommittedFromAwaitingInitializing ==
    /\ phase = "Initializing"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnCommittedFromAwaitingIdle ==
    /\ phase = "Idle"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnCommittedFromAwaitingAttached ==
    /\ phase = "Attached"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnCommittedFromAwaitingRunning ==
    /\ phase = "Running"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnCommittedFromAwaitingRetired ==
    /\ phase = "Retired"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnCommittedFromAwaitingStopped ==
    /\ phase = "Stopped"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnCommittedFromOutputInitializing ==
    /\ phase = "Initializing"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnCommittedFromOutputIdle ==
    /\ phase = "Idle"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnCommittedFromOutputAttached ==
    /\ phase = "Attached"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnCommittedFromOutputRunning ==
    /\ phase = "Running"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnCommittedFromOutputRetired ==
    /\ phase = "Retired"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnCommittedFromOutputStopped ==
    /\ phase = "Stopped"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductOutputStartedFromAwaitingInitializing ==
    /\ phase = "Initializing"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "OutputStarted"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductOutputStartedFromAwaitingIdle ==
    /\ phase = "Idle"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "OutputStarted"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductOutputStartedFromAwaitingAttached ==
    /\ phase = "Attached"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "OutputStarted"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductOutputStartedFromAwaitingRunning ==
    /\ phase = "Running"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "OutputStarted"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductOutputStartedFromAwaitingRetired ==
    /\ phase = "Retired"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "OutputStarted"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductOutputStartedFromAwaitingStopped ==
    /\ phase = "Stopped"
    /\ (realtime_product_turn_phase = "AwaitingProgress")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "OutputStarted"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductOutputStartedFromCommittedInitializing ==
    /\ phase = "Initializing"
    /\ (realtime_product_turn_phase = "Committed")
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductOutputStartedFromCommittedIdle ==
    /\ phase = "Idle"
    /\ (realtime_product_turn_phase = "Committed")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductOutputStartedFromCommittedAttached ==
    /\ phase = "Attached"
    /\ (realtime_product_turn_phase = "Committed")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductOutputStartedFromCommittedRunning ==
    /\ phase = "Running"
    /\ (realtime_product_turn_phase = "Committed")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductOutputStartedFromCommittedRetired ==
    /\ phase = "Retired"
    /\ (realtime_product_turn_phase = "Committed")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductOutputStartedFromCommittedStopped ==
    /\ phase = "Stopped"
    /\ (realtime_product_turn_phase = "Committed")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Preemptible"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnInterruptedFromPreemptibleInitializing ==
    /\ phase = "Initializing"
    /\ (realtime_product_turn_phase = "Preemptible")
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnInterruptedFromPreemptibleIdle ==
    /\ phase = "Idle"
    /\ (realtime_product_turn_phase = "Preemptible")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnInterruptedFromPreemptibleAttached ==
    /\ phase = "Attached"
    /\ (realtime_product_turn_phase = "Preemptible")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnInterruptedFromPreemptibleRunning ==
    /\ phase = "Running"
    /\ (realtime_product_turn_phase = "Preemptible")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnInterruptedFromPreemptibleRetired ==
    /\ phase = "Retired"
    /\ (realtime_product_turn_phase = "Preemptible")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnInterruptedFromPreemptibleStopped ==
    /\ phase = "Stopped"
    /\ (realtime_product_turn_phase = "Preemptible")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Committed"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnInterruptedFromOutputInitializing ==
    /\ phase = "Initializing"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnInterruptedFromOutputIdle ==
    /\ phase = "Idle"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnInterruptedFromOutputAttached ==
    /\ phase = "Attached"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnInterruptedFromOutputRunning ==
    /\ phase = "Running"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnInterruptedFromOutputRetired ==
    /\ phase = "Retired"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnInterruptedFromOutputStopped ==
    /\ phase = "Stopped"
    /\ (realtime_product_turn_phase = "OutputStarted")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "AwaitingProgress"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnTerminalInitializing ==
    /\ phase = "Initializing"
    /\ (realtime_product_turn_phase # "Idle")
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnTerminalIdle ==
    /\ phase = "Idle"
    /\ (realtime_product_turn_phase # "Idle")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnTerminalAttached ==
    /\ phase = "Attached"
    /\ (realtime_product_turn_phase # "Idle")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnTerminalRunning ==
    /\ phase = "Running"
    /\ (realtime_product_turn_phase # "Idle")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnTerminalRetired ==
    /\ phase = "Retired"
    /\ (realtime_product_turn_phase # "Idle")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ProductTurnTerminalStopped ==
    /\ phase = "Stopped"
    /\ (realtime_product_turn_phase # "Idle")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_product_turn_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionAdvanceDuringTurnInitializing(advanced_at_ms) ==
    /\ phase = "Initializing"
    /\ (advanced_at_ms > realtime_projection_frontier_ms)
    /\ (realtime_product_turn_phase # "Idle")
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "StaleDeferred"
    /\ realtime_projection_frontier_ms' = advanced_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionAdvanceDuringTurnIdle(advanced_at_ms) ==
    /\ phase = "Idle"
    /\ (advanced_at_ms > realtime_projection_frontier_ms)
    /\ (realtime_product_turn_phase # "Idle")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "StaleDeferred"
    /\ realtime_projection_frontier_ms' = advanced_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionAdvanceDuringTurnAttached(advanced_at_ms) ==
    /\ phase = "Attached"
    /\ (advanced_at_ms > realtime_projection_frontier_ms)
    /\ (realtime_product_turn_phase # "Idle")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "StaleDeferred"
    /\ realtime_projection_frontier_ms' = advanced_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionAdvanceDuringTurnRunning(advanced_at_ms) ==
    /\ phase = "Running"
    /\ (advanced_at_ms > realtime_projection_frontier_ms)
    /\ (realtime_product_turn_phase # "Idle")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "StaleDeferred"
    /\ realtime_projection_frontier_ms' = advanced_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionAdvanceDuringTurnRetired(advanced_at_ms) ==
    /\ phase = "Retired"
    /\ (advanced_at_ms > realtime_projection_frontier_ms)
    /\ (realtime_product_turn_phase # "Idle")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "StaleDeferred"
    /\ realtime_projection_frontier_ms' = advanced_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionAdvanceDuringTurnStopped(advanced_at_ms) ==
    /\ phase = "Stopped"
    /\ (advanced_at_ms > realtime_projection_frontier_ms)
    /\ (realtime_product_turn_phase # "Idle")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "StaleDeferred"
    /\ realtime_projection_frontier_ms' = advanced_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionAdvanceWhileIdleInitializing(advanced_at_ms) ==
    /\ phase = "Initializing"
    /\ (advanced_at_ms > realtime_projection_frontier_ms)
    /\ (realtime_product_turn_phase = "Idle")
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "StaleImmediate"
    /\ realtime_projection_frontier_ms' = advanced_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionAdvanceWhileIdleIdle(advanced_at_ms) ==
    /\ phase = "Idle"
    /\ (advanced_at_ms > realtime_projection_frontier_ms)
    /\ (realtime_product_turn_phase = "Idle")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "StaleImmediate"
    /\ realtime_projection_frontier_ms' = advanced_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionAdvanceWhileIdleAttached(advanced_at_ms) ==
    /\ phase = "Attached"
    /\ (advanced_at_ms > realtime_projection_frontier_ms)
    /\ (realtime_product_turn_phase = "Idle")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "StaleImmediate"
    /\ realtime_projection_frontier_ms' = advanced_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionAdvanceWhileIdleRunning(advanced_at_ms) ==
    /\ phase = "Running"
    /\ (advanced_at_ms > realtime_projection_frontier_ms)
    /\ (realtime_product_turn_phase = "Idle")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "StaleImmediate"
    /\ realtime_projection_frontier_ms' = advanced_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionAdvanceWhileIdleRetired(advanced_at_ms) ==
    /\ phase = "Retired"
    /\ (advanced_at_ms > realtime_projection_frontier_ms)
    /\ (realtime_product_turn_phase = "Idle")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "StaleImmediate"
    /\ realtime_projection_frontier_ms' = advanced_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionAdvanceWhileIdleStopped(advanced_at_ms) ==
    /\ phase = "Stopped"
    /\ (advanced_at_ms > realtime_projection_frontier_ms)
    /\ (realtime_product_turn_phase = "Idle")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "StaleImmediate"
    /\ realtime_projection_frontier_ms' = advanced_at_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionRefreshedInitializing(observed_ms) ==
    /\ phase = "Initializing"
    /\ (observed_ms >= realtime_projection_frontier_ms)
    /\ ((realtime_projection_freshness # "Clean") \/ (observed_ms > realtime_projection_frontier_ms))
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "Clean"
    /\ realtime_projection_frontier_ms' = IF (observed_ms > realtime_projection_frontier_ms) THEN observed_ms ELSE realtime_projection_frontier_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionRefreshedIdle(observed_ms) ==
    /\ phase = "Idle"
    /\ (observed_ms >= realtime_projection_frontier_ms)
    /\ ((realtime_projection_freshness # "Clean") \/ (observed_ms > realtime_projection_frontier_ms))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "Clean"
    /\ realtime_projection_frontier_ms' = IF (observed_ms > realtime_projection_frontier_ms) THEN observed_ms ELSE realtime_projection_frontier_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionRefreshedAttached(observed_ms) ==
    /\ phase = "Attached"
    /\ (observed_ms >= realtime_projection_frontier_ms)
    /\ ((realtime_projection_freshness # "Clean") \/ (observed_ms > realtime_projection_frontier_ms))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "Clean"
    /\ realtime_projection_frontier_ms' = IF (observed_ms > realtime_projection_frontier_ms) THEN observed_ms ELSE realtime_projection_frontier_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionRefreshedRunning(observed_ms) ==
    /\ phase = "Running"
    /\ (observed_ms >= realtime_projection_frontier_ms)
    /\ ((realtime_projection_freshness # "Clean") \/ (observed_ms > realtime_projection_frontier_ms))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "Clean"
    /\ realtime_projection_frontier_ms' = IF (observed_ms > realtime_projection_frontier_ms) THEN observed_ms ELSE realtime_projection_frontier_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionRefreshedRetired(observed_ms) ==
    /\ phase = "Retired"
    /\ (observed_ms >= realtime_projection_frontier_ms)
    /\ ((realtime_projection_freshness # "Clean") \/ (observed_ms > realtime_projection_frontier_ms))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "Clean"
    /\ realtime_projection_frontier_ms' = IF (observed_ms > realtime_projection_frontier_ms) THEN observed_ms ELSE realtime_projection_frontier_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionRefreshedStopped(observed_ms) ==
    /\ phase = "Stopped"
    /\ (observed_ms >= realtime_projection_frontier_ms)
    /\ ((realtime_projection_freshness # "Clean") \/ (observed_ms > realtime_projection_frontier_ms))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "Clean"
    /\ realtime_projection_frontier_ms' = IF (observed_ms > realtime_projection_frontier_ms) THEN observed_ms ELSE realtime_projection_frontier_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionResetInitializing(baseline_ms) ==
    /\ phase = "Initializing"
    /\ ((realtime_projection_freshness # "Clean") \/ (baseline_ms > realtime_projection_frontier_ms))
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "Clean"
    /\ realtime_projection_frontier_ms' = IF (baseline_ms > realtime_projection_frontier_ms) THEN baseline_ms ELSE realtime_projection_frontier_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionResetIdle(baseline_ms) ==
    /\ phase = "Idle"
    /\ ((realtime_projection_freshness # "Clean") \/ (baseline_ms > realtime_projection_frontier_ms))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "Clean"
    /\ realtime_projection_frontier_ms' = IF (baseline_ms > realtime_projection_frontier_ms) THEN baseline_ms ELSE realtime_projection_frontier_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionResetAttached(baseline_ms) ==
    /\ phase = "Attached"
    /\ ((realtime_projection_freshness # "Clean") \/ (baseline_ms > realtime_projection_frontier_ms))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "Clean"
    /\ realtime_projection_frontier_ms' = IF (baseline_ms > realtime_projection_frontier_ms) THEN baseline_ms ELSE realtime_projection_frontier_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionResetRunning(baseline_ms) ==
    /\ phase = "Running"
    /\ ((realtime_projection_freshness # "Clean") \/ (baseline_ms > realtime_projection_frontier_ms))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "Clean"
    /\ realtime_projection_frontier_ms' = IF (baseline_ms > realtime_projection_frontier_ms) THEN baseline_ms ELSE realtime_projection_frontier_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionResetRetired(baseline_ms) ==
    /\ phase = "Retired"
    /\ ((realtime_projection_freshness # "Clean") \/ (baseline_ms > realtime_projection_frontier_ms))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "Clean"
    /\ realtime_projection_frontier_ms' = IF (baseline_ms > realtime_projection_frontier_ms) THEN baseline_ms ELSE realtime_projection_frontier_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RealtimeProjectionResetStopped(baseline_ms) ==
    /\ phase = "Stopped"
    /\ ((realtime_projection_freshness # "Clean") \/ (baseline_ms > realtime_projection_frontier_ms))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = "Clean"
    /\ realtime_projection_frontier_ms' = IF (baseline_ms > realtime_projection_frontier_ms) THEN baseline_ms ELSE realtime_projection_frontier_ms
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyRealtimeClientInputSubmittedInitializing ==
    /\ phase = "Initializing"
    /\ (realtime_reconnect_policy # "ReattachAndRecover")
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_policy' = "ReattachAndRecover"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyRealtimeClientInputSubmittedIdle ==
    /\ phase = "Idle"
    /\ (realtime_reconnect_policy # "ReattachAndRecover")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_policy' = "ReattachAndRecover"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyRealtimeClientInputSubmittedAttached ==
    /\ phase = "Attached"
    /\ (realtime_reconnect_policy # "ReattachAndRecover")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_policy' = "ReattachAndRecover"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyRealtimeClientInputSubmittedRunning ==
    /\ phase = "Running"
    /\ (realtime_reconnect_policy # "ReattachAndRecover")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_policy' = "ReattachAndRecover"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyRealtimeClientInputSubmittedRetired ==
    /\ phase = "Retired"
    /\ (realtime_reconnect_policy # "ReattachAndRecover")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_policy' = "ReattachAndRecover"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyRealtimeClientInputSubmittedStopped ==
    /\ phase = "Stopped"
    /\ (realtime_reconnect_policy # "ReattachAndRecover")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_policy' = "ReattachAndRecover"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyRealtimeMidTurnActivityInitializing ==
    /\ phase = "Initializing"
    /\ (realtime_reconnect_policy # "ReattachAndRecover")
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_policy' = "ReattachAndRecover"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyRealtimeMidTurnActivityIdle ==
    /\ phase = "Idle"
    /\ (realtime_reconnect_policy # "ReattachAndRecover")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_policy' = "ReattachAndRecover"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyRealtimeMidTurnActivityAttached ==
    /\ phase = "Attached"
    /\ (realtime_reconnect_policy # "ReattachAndRecover")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_policy' = "ReattachAndRecover"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyRealtimeMidTurnActivityRunning ==
    /\ phase = "Running"
    /\ (realtime_reconnect_policy # "ReattachAndRecover")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_policy' = "ReattachAndRecover"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyRealtimeMidTurnActivityRetired ==
    /\ phase = "Retired"
    /\ (realtime_reconnect_policy # "ReattachAndRecover")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_policy' = "ReattachAndRecover"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyRealtimeMidTurnActivityStopped ==
    /\ phase = "Stopped"
    /\ (realtime_reconnect_policy # "ReattachAndRecover")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_reconnect_policy' = "ReattachAndRecover"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyRealtimeTurnTerminatedInitializing ==
    /\ phase = "Initializing"
    /\ ((realtime_reconnect_policy # "CleanExit") \/ (realtime_projection_freshness = "StaleDeferred"))
    /\ phase' = "Initializing"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = IF (realtime_projection_freshness = "StaleDeferred") THEN "StaleImmediate" ELSE realtime_projection_freshness
    /\ realtime_reconnect_policy' = "CleanExit"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_frontier_ms, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyRealtimeTurnTerminatedIdle ==
    /\ phase = "Idle"
    /\ ((realtime_reconnect_policy # "CleanExit") \/ (realtime_projection_freshness = "StaleDeferred"))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = IF (realtime_projection_freshness = "StaleDeferred") THEN "StaleImmediate" ELSE realtime_projection_freshness
    /\ realtime_reconnect_policy' = "CleanExit"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_frontier_ms, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyRealtimeTurnTerminatedAttached ==
    /\ phase = "Attached"
    /\ ((realtime_reconnect_policy # "CleanExit") \/ (realtime_projection_freshness = "StaleDeferred"))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = IF (realtime_projection_freshness = "StaleDeferred") THEN "StaleImmediate" ELSE realtime_projection_freshness
    /\ realtime_reconnect_policy' = "CleanExit"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_frontier_ms, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyRealtimeTurnTerminatedRunning ==
    /\ phase = "Running"
    /\ ((realtime_reconnect_policy # "CleanExit") \/ (realtime_projection_freshness = "StaleDeferred"))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = IF (realtime_projection_freshness = "StaleDeferred") THEN "StaleImmediate" ELSE realtime_projection_freshness
    /\ realtime_reconnect_policy' = "CleanExit"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_frontier_ms, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyRealtimeTurnTerminatedRetired ==
    /\ phase = "Retired"
    /\ ((realtime_reconnect_policy # "CleanExit") \/ (realtime_projection_freshness = "StaleDeferred"))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = IF (realtime_projection_freshness = "StaleDeferred") THEN "StaleImmediate" ELSE realtime_projection_freshness
    /\ realtime_reconnect_policy' = "CleanExit"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_frontier_ms, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClassifyRealtimeTurnTerminatedStopped ==
    /\ phase = "Stopped"
    /\ ((realtime_reconnect_policy # "CleanExit") \/ (realtime_projection_freshness = "StaleDeferred"))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ realtime_projection_freshness' = IF (realtime_projection_freshness = "StaleDeferred") THEN "StaleImmediate" ELSE realtime_projection_freshness
    /\ realtime_reconnect_policy' = "CleanExit"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_frontier_ms, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


BeginLiveTopologyReconfigureIdle(authority_epoch) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Reconfiguring"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


BeginLiveTopologyReconfigureAttached(authority_epoch) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Reconfiguring"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


BeginLiveTopologyReconfigureRunning(authority_epoch) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Reconfiguring"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


BeginLiveTopologyReconfigureRetired(authority_epoch) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Reconfiguring"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


BeginLiveTopologyReconfigureStopped(authority_epoch) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (realtime_binding_authority_epoch = Some(authority_epoch))
    /\ (live_topology_phase = "Idle")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Reconfiguring"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ApplyLiveTopologyIdentityIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (live_topology_phase = "Detached")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostIdentityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ApplyLiveTopologyIdentityAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (live_topology_phase = "Detached")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostIdentityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ApplyLiveTopologyIdentityRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (live_topology_phase = "Detached")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostIdentityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ApplyLiveTopologyIdentityRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (live_topology_phase = "Detached")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostIdentityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ApplyLiveTopologyIdentityStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (live_topology_phase = "Detached")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostIdentityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ApplyLiveTopologyVisibilityIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostIdentityApplied")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostVisibilityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ApplyLiveTopologyVisibilityAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostIdentityApplied")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostVisibilityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ApplyLiveTopologyVisibilityRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostIdentityApplied")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostVisibilityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ApplyLiveTopologyVisibilityRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostIdentityApplied")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostVisibilityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ApplyLiveTopologyVisibilityStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostIdentityApplied")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "HostVisibilityApplied"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


CompleteLiveTopologyIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostVisibilityApplied")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


CompleteLiveTopologyAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostVisibilityApplied")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


CompleteLiveTopologyRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostVisibilityApplied")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


CompleteLiveTopologyRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostVisibilityApplied")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


CompleteLiveTopologyStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (live_topology_phase = "HostVisibilityApplied")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AbortLiveTopologyBeforeDetachIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AbortLiveTopologyBeforeDetachAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AbortLiveTopologyBeforeDetachRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AbortLiveTopologyBeforeDetachRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AbortLiveTopologyBeforeDetachStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (live_topology_phase = "Reconfiguring")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ live_topology_phase' = "Idle"
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


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
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AttachSessionIngressIdle(comms_runtime_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (peer_ingress_owner_kind = "Unattached")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "SessionOwned"
    /\ peer_ingress_comms_runtime_id' = Some(comms_runtime_id)
    /\ peer_ingress_mob_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AttachSessionIngressAttached(comms_runtime_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (peer_ingress_owner_kind = "Unattached")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "SessionOwned"
    /\ peer_ingress_comms_runtime_id' = Some(comms_runtime_id)
    /\ peer_ingress_mob_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AttachSessionIngressRunning(comms_runtime_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (peer_ingress_owner_kind = "Unattached")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "SessionOwned"
    /\ peer_ingress_comms_runtime_id' = Some(comms_runtime_id)
    /\ peer_ingress_mob_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AttachSessionIngressRetired(comms_runtime_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (peer_ingress_owner_kind = "Unattached")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "SessionOwned"
    /\ peer_ingress_comms_runtime_id' = Some(comms_runtime_id)
    /\ peer_ingress_mob_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AttachSessionIngressStopped(comms_runtime_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (peer_ingress_owner_kind = "Unattached")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "SessionOwned"
    /\ peer_ingress_comms_runtime_id' = Some(comms_runtime_id)
    /\ peer_ingress_mob_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AttachMobIngressIdle(comms_runtime_id, mob_id) ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ ((peer_ingress_owner_kind = "Unattached") \/ (peer_ingress_owner_kind = "SessionOwned"))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "MobOwned"
    /\ peer_ingress_comms_runtime_id' = Some(comms_runtime_id)
    /\ peer_ingress_mob_id' = Some(mob_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AttachMobIngressAttached(comms_runtime_id, mob_id) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ ((peer_ingress_owner_kind = "Unattached") \/ (peer_ingress_owner_kind = "SessionOwned"))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "MobOwned"
    /\ peer_ingress_comms_runtime_id' = Some(comms_runtime_id)
    /\ peer_ingress_mob_id' = Some(mob_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AttachMobIngressRunning(comms_runtime_id, mob_id) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ ((peer_ingress_owner_kind = "Unattached") \/ (peer_ingress_owner_kind = "SessionOwned"))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "MobOwned"
    /\ peer_ingress_comms_runtime_id' = Some(comms_runtime_id)
    /\ peer_ingress_mob_id' = Some(mob_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AttachMobIngressRetired(comms_runtime_id, mob_id) ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ ((peer_ingress_owner_kind = "Unattached") \/ (peer_ingress_owner_kind = "SessionOwned"))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "MobOwned"
    /\ peer_ingress_comms_runtime_id' = Some(comms_runtime_id)
    /\ peer_ingress_mob_id' = Some(mob_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AttachMobIngressStopped(comms_runtime_id, mob_id) ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ ((peer_ingress_owner_kind = "Unattached") \/ (peer_ingress_owner_kind = "SessionOwned"))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "MobOwned"
    /\ peer_ingress_comms_runtime_id' = Some(comms_runtime_id)
    /\ peer_ingress_mob_id' = Some(mob_id)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


DetachIngressIdle ==
    /\ phase = "Idle"
    /\ (session_id # None)
    /\ (peer_ingress_owner_kind # "Unattached")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "Unattached"
    /\ peer_ingress_comms_runtime_id' = None
    /\ peer_ingress_mob_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


DetachIngressAttached ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ (peer_ingress_owner_kind # "Unattached")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "Unattached"
    /\ peer_ingress_comms_runtime_id' = None
    /\ peer_ingress_mob_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


DetachIngressRunning ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ (peer_ingress_owner_kind # "Unattached")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "Unattached"
    /\ peer_ingress_comms_runtime_id' = None
    /\ peer_ingress_mob_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


DetachIngressRetired ==
    /\ phase = "Retired"
    /\ (session_id # None)
    /\ (peer_ingress_owner_kind # "Unattached")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "Unattached"
    /\ peer_ingress_comms_runtime_id' = None
    /\ peer_ingress_mob_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


DetachIngressStopped ==
    /\ phase = "Stopped"
    /\ (session_id # None)
    /\ (peer_ingress_owner_kind # "Unattached")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ peer_ingress_owner_kind' = "Unattached"
    /\ peer_ingress_comms_runtime_id' = None
    /\ peer_ingress_mob_id' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


BindSupervisorIdle(name, peer_id, address, epoch) ==
    /\ phase = "Idle"
    /\ (supervisor_binding_kind = "Unbound")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ supervisor_binding_kind' = "Bound"
    /\ supervisor_bound_name' = Some(name)
    /\ supervisor_bound_peer_id' = Some(peer_id)
    /\ supervisor_bound_address' = Some(address)
    /\ supervisor_bound_epoch' = Some(epoch)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


BindSupervisorAttached(name, peer_id, address, epoch) ==
    /\ phase = "Attached"
    /\ (supervisor_binding_kind = "Unbound")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ supervisor_binding_kind' = "Bound"
    /\ supervisor_bound_name' = Some(name)
    /\ supervisor_bound_peer_id' = Some(peer_id)
    /\ supervisor_bound_address' = Some(address)
    /\ supervisor_bound_epoch' = Some(epoch)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


BindSupervisorRunning(name, peer_id, address, epoch) ==
    /\ phase = "Running"
    /\ (supervisor_binding_kind = "Unbound")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ supervisor_binding_kind' = "Bound"
    /\ supervisor_bound_name' = Some(name)
    /\ supervisor_bound_peer_id' = Some(peer_id)
    /\ supervisor_bound_address' = Some(address)
    /\ supervisor_bound_epoch' = Some(epoch)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


BindSupervisorRetired(name, peer_id, address, epoch) ==
    /\ phase = "Retired"
    /\ (supervisor_binding_kind = "Unbound")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ supervisor_binding_kind' = "Bound"
    /\ supervisor_bound_name' = Some(name)
    /\ supervisor_bound_peer_id' = Some(peer_id)
    /\ supervisor_bound_address' = Some(address)
    /\ supervisor_bound_epoch' = Some(epoch)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


BindSupervisorStopped(name, peer_id, address, epoch) ==
    /\ phase = "Stopped"
    /\ (supervisor_binding_kind = "Unbound")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ supervisor_binding_kind' = "Bound"
    /\ supervisor_bound_name' = Some(name)
    /\ supervisor_bound_peer_id' = Some(peer_id)
    /\ supervisor_bound_address' = Some(address)
    /\ supervisor_bound_epoch' = Some(epoch)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AuthorizeSupervisorIdle(name, peer_id, address, epoch) ==
    /\ phase = "Idle"
    /\ (supervisor_binding_kind = "Bound")
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ supervisor_bound_name' = Some(name)
    /\ supervisor_bound_peer_id' = Some(peer_id)
    /\ supervisor_bound_address' = Some(address)
    /\ supervisor_bound_epoch' = Some(epoch)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AuthorizeSupervisorAttached(name, peer_id, address, epoch) ==
    /\ phase = "Attached"
    /\ (supervisor_binding_kind = "Bound")
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ supervisor_bound_name' = Some(name)
    /\ supervisor_bound_peer_id' = Some(peer_id)
    /\ supervisor_bound_address' = Some(address)
    /\ supervisor_bound_epoch' = Some(epoch)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AuthorizeSupervisorRunning(name, peer_id, address, epoch) ==
    /\ phase = "Running"
    /\ (supervisor_binding_kind = "Bound")
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ supervisor_bound_name' = Some(name)
    /\ supervisor_bound_peer_id' = Some(peer_id)
    /\ supervisor_bound_address' = Some(address)
    /\ supervisor_bound_epoch' = Some(epoch)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AuthorizeSupervisorRetired(name, peer_id, address, epoch) ==
    /\ phase = "Retired"
    /\ (supervisor_binding_kind = "Bound")
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ supervisor_bound_name' = Some(name)
    /\ supervisor_bound_peer_id' = Some(peer_id)
    /\ supervisor_bound_address' = Some(address)
    /\ supervisor_bound_epoch' = Some(epoch)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AuthorizeSupervisorStopped(name, peer_id, address, epoch) ==
    /\ phase = "Stopped"
    /\ (supervisor_binding_kind = "Bound")
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ supervisor_bound_name' = Some(name)
    /\ supervisor_bound_peer_id' = Some(peer_id)
    /\ supervisor_bound_address' = Some(address)
    /\ supervisor_bound_epoch' = Some(epoch)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RevokeSupervisorIdle(peer_id, epoch) ==
    /\ phase = "Idle"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ supervisor_binding_kind' = "Unbound"
    /\ supervisor_bound_name' = None
    /\ supervisor_bound_peer_id' = None
    /\ supervisor_bound_address' = None
    /\ supervisor_bound_epoch' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RevokeSupervisorAttached(peer_id, epoch) ==
    /\ phase = "Attached"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ supervisor_binding_kind' = "Unbound"
    /\ supervisor_bound_name' = None
    /\ supervisor_bound_peer_id' = None
    /\ supervisor_bound_address' = None
    /\ supervisor_bound_epoch' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RevokeSupervisorRunning(peer_id, epoch) ==
    /\ phase = "Running"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ supervisor_binding_kind' = "Unbound"
    /\ supervisor_bound_name' = None
    /\ supervisor_bound_peer_id' = None
    /\ supervisor_bound_address' = None
    /\ supervisor_bound_epoch' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RevokeSupervisorRetired(peer_id, epoch) ==
    /\ phase = "Retired"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ supervisor_binding_kind' = "Unbound"
    /\ supervisor_bound_name' = None
    /\ supervisor_bound_peer_id' = None
    /\ supervisor_bound_address' = None
    /\ supervisor_bound_epoch' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


RevokeSupervisorStopped(peer_id, epoch) ==
    /\ phase = "Stopped"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ supervisor_binding_kind' = "Unbound"
    /\ supervisor_bound_name' = None
    /\ supervisor_bound_peer_id' = None
    /\ supervisor_bound_address' = None
    /\ supervisor_bound_epoch' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SupervisorTrustEdgePublishedIdle(peer_id, epoch) ==
    /\ phase = "Idle"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SupervisorTrustEdgePublishedAttached(peer_id, epoch) ==
    /\ phase = "Attached"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SupervisorTrustEdgePublishedRunning(peer_id, epoch) ==
    /\ phase = "Running"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SupervisorTrustEdgePublishedRetired(peer_id, epoch) ==
    /\ phase = "Retired"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SupervisorTrustEdgePublishedStopped(peer_id, epoch) ==
    /\ phase = "Stopped"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SupervisorTrustEdgePublishFailedIdle(peer_id, epoch, reason) ==
    /\ phase = "Idle"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SupervisorTrustEdgePublishFailedAttached(peer_id, epoch, reason) ==
    /\ phase = "Attached"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SupervisorTrustEdgePublishFailedRunning(peer_id, epoch, reason) ==
    /\ phase = "Running"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SupervisorTrustEdgePublishFailedRetired(peer_id, epoch, reason) ==
    /\ phase = "Retired"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SupervisorTrustEdgePublishFailedStopped(peer_id, epoch, reason) ==
    /\ phase = "Stopped"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SupervisorTrustEdgeRevokedIdle(peer_id, epoch) ==
    /\ phase = "Idle"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SupervisorTrustEdgeRevokedAttached(peer_id, epoch) ==
    /\ phase = "Attached"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SupervisorTrustEdgeRevokedRunning(peer_id, epoch) ==
    /\ phase = "Running"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SupervisorTrustEdgeRevokedRetired(peer_id, epoch) ==
    /\ phase = "Retired"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SupervisorTrustEdgeRevokedStopped(peer_id, epoch) ==
    /\ phase = "Stopped"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SupervisorTrustEdgeRevokeFailedIdle(peer_id, epoch, reason) ==
    /\ phase = "Idle"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SupervisorTrustEdgeRevokeFailedAttached(peer_id, epoch, reason) ==
    /\ phase = "Attached"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SupervisorTrustEdgeRevokeFailedRunning(peer_id, epoch, reason) ==
    /\ phase = "Running"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SupervisorTrustEdgeRevokeFailedRetired(peer_id, epoch, reason) ==
    /\ phase = "Retired"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Retired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


SupervisorTrustEdgeRevokeFailedStopped(peer_id, epoch, reason) ==
    /\ phase = "Stopped"
    /\ (supervisor_binding_kind = "Bound")
    /\ (supervisor_bound_peer_id = Some(peer_id))
    /\ (supervisor_bound_epoch = Some(epoch))
    /\ phase' = "Stopped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


OpsBarrierSatisfiedAttached(operation_ids) ==
    /\ phase = "Attached"
    /\ (session_id # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


OpsBarrierSatisfiedRunning(operation_ids) ==
    /\ phase = "Running"
    /\ (session_id # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PublishLocalEndpointIdle(endpoint) ==
    /\ phase = "Idle"
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ local_endpoint' = Some(endpoint)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PublishLocalEndpointAttached(endpoint) ==
    /\ phase = "Attached"
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ local_endpoint' = Some(endpoint)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


PublishLocalEndpointRunning(endpoint) ==
    /\ phase = "Running"
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ local_endpoint' = Some(endpoint)
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClearLocalEndpointIdle ==
    /\ phase = "Idle"
    /\ (local_endpoint # None)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ local_endpoint' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClearLocalEndpointAttached ==
    /\ phase = "Attached"
    /\ (local_endpoint # None)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ local_endpoint' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


ClearLocalEndpointRunning ==
    /\ phase = "Running"
    /\ (local_endpoint # None)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ local_endpoint' = None
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, direct_peer_endpoints, mob_overlay_peer_endpoints, peer_projection_epoch, mob_overlay_epoch >>


AddDirectPeerEndpointIdle(endpoint) ==
    /\ phase = "Idle"
    /\ ((endpoint \in direct_peer_endpoints) = FALSE)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ direct_peer_endpoints' = (direct_peer_endpoints \cup {endpoint})
    /\ peer_projection_epoch' = (peer_projection_epoch) + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, mob_overlay_peer_endpoints, mob_overlay_epoch >>


AddDirectPeerEndpointAttached(endpoint) ==
    /\ phase = "Attached"
    /\ ((endpoint \in direct_peer_endpoints) = FALSE)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ direct_peer_endpoints' = (direct_peer_endpoints \cup {endpoint})
    /\ peer_projection_epoch' = (peer_projection_epoch) + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, mob_overlay_peer_endpoints, mob_overlay_epoch >>


AddDirectPeerEndpointRunning(endpoint) ==
    /\ phase = "Running"
    /\ ((endpoint \in direct_peer_endpoints) = FALSE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ direct_peer_endpoints' = (direct_peer_endpoints \cup {endpoint})
    /\ peer_projection_epoch' = (peer_projection_epoch) + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, mob_overlay_peer_endpoints, mob_overlay_epoch >>


RemoveDirectPeerEndpointIdle(endpoint) ==
    /\ phase = "Idle"
    /\ ((endpoint \in direct_peer_endpoints) = TRUE)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ direct_peer_endpoints' = (direct_peer_endpoints \ {endpoint})
    /\ peer_projection_epoch' = (peer_projection_epoch) + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, mob_overlay_peer_endpoints, mob_overlay_epoch >>


RemoveDirectPeerEndpointAttached(endpoint) ==
    /\ phase = "Attached"
    /\ ((endpoint \in direct_peer_endpoints) = TRUE)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ direct_peer_endpoints' = (direct_peer_endpoints \ {endpoint})
    /\ peer_projection_epoch' = (peer_projection_epoch) + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, mob_overlay_peer_endpoints, mob_overlay_epoch >>


RemoveDirectPeerEndpointRunning(endpoint) ==
    /\ phase = "Running"
    /\ ((endpoint \in direct_peer_endpoints) = TRUE)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ direct_peer_endpoints' = (direct_peer_endpoints \ {endpoint})
    /\ peer_projection_epoch' = (peer_projection_epoch) + 1
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, mob_overlay_peer_endpoints, mob_overlay_epoch >>


ApplyMobPeerOverlayIdle(epoch, endpoints) ==
    /\ phase = "Idle"
    /\ (epoch > mob_overlay_epoch)
    /\ phase' = "Idle"
    /\ model_step_count' = model_step_count + 1
    /\ mob_overlay_peer_endpoints' = endpoints
    /\ peer_projection_epoch' = (peer_projection_epoch) + 1
    /\ mob_overlay_epoch' = epoch
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints >>


ApplyMobPeerOverlayAttached(epoch, endpoints) ==
    /\ phase = "Attached"
    /\ (epoch > mob_overlay_epoch)
    /\ phase' = "Attached"
    /\ model_step_count' = model_step_count + 1
    /\ mob_overlay_peer_endpoints' = endpoints
    /\ peer_projection_epoch' = (peer_projection_epoch) + 1
    /\ mob_overlay_epoch' = epoch
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints >>


ApplyMobPeerOverlayRunning(epoch, endpoints) ==
    /\ phase = "Running"
    /\ (epoch > mob_overlay_epoch)
    /\ phase' = "Running"
    /\ model_step_count' = model_step_count + 1
    /\ mob_overlay_peer_endpoints' = endpoints
    /\ peer_projection_epoch' = (peer_projection_epoch) + 1
    /\ mob_overlay_epoch' = epoch
    /\ UNCHANGED << session_id, active_runtime_id, active_fence_token, current_run_id, pre_run_phase, silent_intent_overrides, realtime_intent_present, realtime_binding_state, realtime_binding_authority_epoch, realtime_reattach_required, realtime_next_authority_epoch, realtime_reconnect_attempt_count, realtime_reconnect_next_retry_at_ms, realtime_reconnect_deadline_at_ms, live_topology_phase, mcp_server_states, pending_peer_requests, inbound_peer_requests, last_session_context_updated_at_ms, reserved_interaction_streams, attached_interaction_streams, realtime_product_turn_phase, realtime_projection_freshness, realtime_projection_frontier_ms, realtime_reconnect_policy, peer_ingress_owner_kind, peer_ingress_comms_runtime_id, peer_ingress_mob_id, supervisor_binding_kind, supervisor_bound_name, supervisor_bound_peer_id, supervisor_bound_address, supervisor_bound_epoch, local_endpoint, direct_peer_endpoints >>


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
    \/ \E runtime_id \in AgentRuntimeIdValues : \E work_id \in WorkIdValues : \E origin \in WorkOriginValues : IngestIdle(runtime_id, work_id, origin)
    \/ \E runtime_id \in AgentRuntimeIdValues : \E work_id \in WorkIdValues : \E origin \in WorkOriginValues : IngestAttached(runtime_id, work_id, origin)
    \/ \E runtime_id \in AgentRuntimeIdValues : \E work_id \in WorkIdValues : \E origin \in WorkOriginValues : IngestRunning(runtime_id, work_id, origin)
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
    \/ \E attempt_count \in 0..2 : \E next_retry_at_ms \in OptionU64Values : \E deadline_at_ms \in OptionU64Values : ProjectRealtimeReconnectProgressIdle(attempt_count, next_retry_at_ms, deadline_at_ms)
    \/ \E attempt_count \in 0..2 : \E next_retry_at_ms \in OptionU64Values : \E deadline_at_ms \in OptionU64Values : ProjectRealtimeReconnectProgressAttached(attempt_count, next_retry_at_ms, deadline_at_ms)
    \/ \E attempt_count \in 0..2 : \E next_retry_at_ms \in OptionU64Values : \E deadline_at_ms \in OptionU64Values : ProjectRealtimeReconnectProgressRunning(attempt_count, next_retry_at_ms, deadline_at_ms)
    \/ \E attempt_count \in 0..2 : \E next_retry_at_ms \in OptionU64Values : \E deadline_at_ms \in OptionU64Values : ProjectRealtimeReconnectProgressRetired(attempt_count, next_retry_at_ms, deadline_at_ms)
    \/ \E attempt_count \in 0..2 : \E next_retry_at_ms \in OptionU64Values : \E deadline_at_ms \in OptionU64Values : ProjectRealtimeReconnectProgressStopped(attempt_count, next_retry_at_ms, deadline_at_ms)
    \/ ClearRealtimeReconnectProgressIdle
    \/ ClearRealtimeReconnectProgressAttached
    \/ ClearRealtimeReconnectProgressRunning
    \/ ClearRealtimeReconnectProgressRetired
    \/ ClearRealtimeReconnectProgressStopped
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
    \/ \E advanced_at_ms \in 0..2 : RealtimeProjectionAdvanceDuringTurnInitializing(advanced_at_ms)
    \/ \E advanced_at_ms \in 0..2 : RealtimeProjectionAdvanceDuringTurnIdle(advanced_at_ms)
    \/ \E advanced_at_ms \in 0..2 : RealtimeProjectionAdvanceDuringTurnAttached(advanced_at_ms)
    \/ \E advanced_at_ms \in 0..2 : RealtimeProjectionAdvanceDuringTurnRunning(advanced_at_ms)
    \/ \E advanced_at_ms \in 0..2 : RealtimeProjectionAdvanceDuringTurnRetired(advanced_at_ms)
    \/ \E advanced_at_ms \in 0..2 : RealtimeProjectionAdvanceDuringTurnStopped(advanced_at_ms)
    \/ \E advanced_at_ms \in 0..2 : RealtimeProjectionAdvanceWhileIdleInitializing(advanced_at_ms)
    \/ \E advanced_at_ms \in 0..2 : RealtimeProjectionAdvanceWhileIdleIdle(advanced_at_ms)
    \/ \E advanced_at_ms \in 0..2 : RealtimeProjectionAdvanceWhileIdleAttached(advanced_at_ms)
    \/ \E advanced_at_ms \in 0..2 : RealtimeProjectionAdvanceWhileIdleRunning(advanced_at_ms)
    \/ \E advanced_at_ms \in 0..2 : RealtimeProjectionAdvanceWhileIdleRetired(advanced_at_ms)
    \/ \E advanced_at_ms \in 0..2 : RealtimeProjectionAdvanceWhileIdleStopped(advanced_at_ms)
    \/ \E observed_ms \in 0..2 : RealtimeProjectionRefreshedInitializing(observed_ms)
    \/ \E observed_ms \in 0..2 : RealtimeProjectionRefreshedIdle(observed_ms)
    \/ \E observed_ms \in 0..2 : RealtimeProjectionRefreshedAttached(observed_ms)
    \/ \E observed_ms \in 0..2 : RealtimeProjectionRefreshedRunning(observed_ms)
    \/ \E observed_ms \in 0..2 : RealtimeProjectionRefreshedRetired(observed_ms)
    \/ \E observed_ms \in 0..2 : RealtimeProjectionRefreshedStopped(observed_ms)
    \/ \E baseline_ms \in 0..2 : RealtimeProjectionResetInitializing(baseline_ms)
    \/ \E baseline_ms \in 0..2 : RealtimeProjectionResetIdle(baseline_ms)
    \/ \E baseline_ms \in 0..2 : RealtimeProjectionResetAttached(baseline_ms)
    \/ \E baseline_ms \in 0..2 : RealtimeProjectionResetRunning(baseline_ms)
    \/ \E baseline_ms \in 0..2 : RealtimeProjectionResetRetired(baseline_ms)
    \/ \E baseline_ms \in 0..2 : RealtimeProjectionResetStopped(baseline_ms)
    \/ ClassifyRealtimeClientInputSubmittedInitializing
    \/ ClassifyRealtimeClientInputSubmittedIdle
    \/ ClassifyRealtimeClientInputSubmittedAttached
    \/ ClassifyRealtimeClientInputSubmittedRunning
    \/ ClassifyRealtimeClientInputSubmittedRetired
    \/ ClassifyRealtimeClientInputSubmittedStopped
    \/ ClassifyRealtimeMidTurnActivityInitializing
    \/ ClassifyRealtimeMidTurnActivityIdle
    \/ ClassifyRealtimeMidTurnActivityAttached
    \/ ClassifyRealtimeMidTurnActivityRunning
    \/ ClassifyRealtimeMidTurnActivityRetired
    \/ ClassifyRealtimeMidTurnActivityStopped
    \/ ClassifyRealtimeTurnTerminatedInitializing
    \/ ClassifyRealtimeTurnTerminatedIdle
    \/ ClassifyRealtimeTurnTerminatedAttached
    \/ ClassifyRealtimeTurnTerminatedRunning
    \/ ClassifyRealtimeTurnTerminatedRetired
    \/ ClassifyRealtimeTurnTerminatedStopped
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
    \/ \E name \in StringValues : \E peer_id \in StringValues : \E address \in StringValues : \E epoch \in 0..2 : BindSupervisorIdle(name, peer_id, address, epoch)
    \/ \E name \in StringValues : \E peer_id \in StringValues : \E address \in StringValues : \E epoch \in 0..2 : BindSupervisorAttached(name, peer_id, address, epoch)
    \/ \E name \in StringValues : \E peer_id \in StringValues : \E address \in StringValues : \E epoch \in 0..2 : BindSupervisorRunning(name, peer_id, address, epoch)
    \/ \E name \in StringValues : \E peer_id \in StringValues : \E address \in StringValues : \E epoch \in 0..2 : BindSupervisorRetired(name, peer_id, address, epoch)
    \/ \E name \in StringValues : \E peer_id \in StringValues : \E address \in StringValues : \E epoch \in 0..2 : BindSupervisorStopped(name, peer_id, address, epoch)
    \/ \E name \in StringValues : \E peer_id \in StringValues : \E address \in StringValues : \E epoch \in 0..2 : AuthorizeSupervisorIdle(name, peer_id, address, epoch)
    \/ \E name \in StringValues : \E peer_id \in StringValues : \E address \in StringValues : \E epoch \in 0..2 : AuthorizeSupervisorAttached(name, peer_id, address, epoch)
    \/ \E name \in StringValues : \E peer_id \in StringValues : \E address \in StringValues : \E epoch \in 0..2 : AuthorizeSupervisorRunning(name, peer_id, address, epoch)
    \/ \E name \in StringValues : \E peer_id \in StringValues : \E address \in StringValues : \E epoch \in 0..2 : AuthorizeSupervisorRetired(name, peer_id, address, epoch)
    \/ \E name \in StringValues : \E peer_id \in StringValues : \E address \in StringValues : \E epoch \in 0..2 : AuthorizeSupervisorStopped(name, peer_id, address, epoch)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : RevokeSupervisorIdle(peer_id, epoch)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : RevokeSupervisorAttached(peer_id, epoch)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : RevokeSupervisorRunning(peer_id, epoch)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : RevokeSupervisorRetired(peer_id, epoch)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : RevokeSupervisorStopped(peer_id, epoch)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : SupervisorTrustEdgePublishedIdle(peer_id, epoch)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : SupervisorTrustEdgePublishedAttached(peer_id, epoch)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : SupervisorTrustEdgePublishedRunning(peer_id, epoch)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : SupervisorTrustEdgePublishedRetired(peer_id, epoch)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : SupervisorTrustEdgePublishedStopped(peer_id, epoch)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : \E reason \in StringValues : SupervisorTrustEdgePublishFailedIdle(peer_id, epoch, reason)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : \E reason \in StringValues : SupervisorTrustEdgePublishFailedAttached(peer_id, epoch, reason)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : \E reason \in StringValues : SupervisorTrustEdgePublishFailedRunning(peer_id, epoch, reason)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : \E reason \in StringValues : SupervisorTrustEdgePublishFailedRetired(peer_id, epoch, reason)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : \E reason \in StringValues : SupervisorTrustEdgePublishFailedStopped(peer_id, epoch, reason)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : SupervisorTrustEdgeRevokedIdle(peer_id, epoch)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : SupervisorTrustEdgeRevokedAttached(peer_id, epoch)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : SupervisorTrustEdgeRevokedRunning(peer_id, epoch)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : SupervisorTrustEdgeRevokedRetired(peer_id, epoch)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : SupervisorTrustEdgeRevokedStopped(peer_id, epoch)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : \E reason \in StringValues : SupervisorTrustEdgeRevokeFailedIdle(peer_id, epoch, reason)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : \E reason \in StringValues : SupervisorTrustEdgeRevokeFailedAttached(peer_id, epoch, reason)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : \E reason \in StringValues : SupervisorTrustEdgeRevokeFailedRunning(peer_id, epoch, reason)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : \E reason \in StringValues : SupervisorTrustEdgeRevokeFailedRetired(peer_id, epoch, reason)
    \/ \E peer_id \in StringValues : \E epoch \in 0..2 : \E reason \in StringValues : SupervisorTrustEdgeRevokeFailedStopped(peer_id, epoch, reason)
    \/ \E operation_ids \in SetOfOperationIdValues : OpsBarrierSatisfiedAttached(operation_ids)
    \/ \E operation_ids \in SetOfOperationIdValues : OpsBarrierSatisfiedRunning(operation_ids)
    \/ \E endpoint \in PeerEndpointValues : PublishLocalEndpointIdle(endpoint)
    \/ \E endpoint \in PeerEndpointValues : PublishLocalEndpointAttached(endpoint)
    \/ \E endpoint \in PeerEndpointValues : PublishLocalEndpointRunning(endpoint)
    \/ ClearLocalEndpointIdle
    \/ ClearLocalEndpointAttached
    \/ ClearLocalEndpointRunning
    \/ \E endpoint \in PeerEndpointValues : AddDirectPeerEndpointIdle(endpoint)
    \/ \E endpoint \in PeerEndpointValues : AddDirectPeerEndpointAttached(endpoint)
    \/ \E endpoint \in PeerEndpointValues : AddDirectPeerEndpointRunning(endpoint)
    \/ \E endpoint \in PeerEndpointValues : RemoveDirectPeerEndpointIdle(endpoint)
    \/ \E endpoint \in PeerEndpointValues : RemoveDirectPeerEndpointAttached(endpoint)
    \/ \E endpoint \in PeerEndpointValues : RemoveDirectPeerEndpointRunning(endpoint)
    \/ \E epoch \in 0..2 : \E endpoints \in SetOfPeerEndpointValues : ApplyMobPeerOverlayIdle(epoch, endpoints)
    \/ \E epoch \in 0..2 : \E endpoints \in SetOfPeerEndpointValues : ApplyMobPeerOverlayAttached(epoch, endpoints)
    \/ \E epoch \in 0..2 : \E endpoints \in SetOfPeerEndpointValues : ApplyMobPeerOverlayRunning(epoch, endpoints)
    \/ TerminalStutter

fence_requires_bound_runtime == ((active_fence_token = None) \/ (active_runtime_id # None))
running_has_current_run == ((phase # "Running") \/ (current_run_id # None))
current_run_only_while_running_or_retired == ((current_run_id = None) \/ (phase = "Running") \/ (phase = "Retired"))
realtime_binding_epoch_consistency == ((realtime_binding_state = "Unbound") = (realtime_binding_authority_epoch = None))
peer_ingress_owner_consistency == (((peer_ingress_owner_kind = "Unattached") /\ (peer_ingress_comms_runtime_id = None) /\ (peer_ingress_mob_id = None)) \/ ((peer_ingress_owner_kind = "SessionOwned") /\ (peer_ingress_comms_runtime_id # None) /\ (peer_ingress_mob_id = None)) \/ ((peer_ingress_owner_kind = "MobOwned") /\ (peer_ingress_comms_runtime_id # None) /\ (peer_ingress_mob_id # None)))
supervisor_binding_consistency == (((supervisor_binding_kind = "Unbound") /\ (supervisor_bound_name = None) /\ (supervisor_bound_peer_id = None) /\ (supervisor_bound_address = None) /\ (supervisor_bound_epoch = None)) \/ ((supervisor_binding_kind = "Bound") /\ (supervisor_bound_name # None) /\ (supervisor_bound_peer_id # None) /\ (supervisor_bound_address # None) /\ (supervisor_bound_epoch # None)))

CiStateConstraint == /\ model_step_count <= 6 /\ Cardinality(silent_intent_overrides) <= 1 /\ Cardinality(DOMAIN mcp_server_states) <= 1 /\ Cardinality(DOMAIN pending_peer_requests) <= 1 /\ Cardinality(DOMAIN inbound_peer_requests) <= 1 /\ Cardinality(reserved_interaction_streams) <= 1 /\ Cardinality(attached_interaction_streams) <= 1 /\ Cardinality(direct_peer_endpoints) <= 1 /\ Cardinality(mob_overlay_peer_endpoints) <= 1
DeepStateConstraint == /\ model_step_count <= 8 /\ Cardinality(silent_intent_overrides) <= 2 /\ Cardinality(DOMAIN mcp_server_states) <= 2 /\ Cardinality(DOMAIN pending_peer_requests) <= 2 /\ Cardinality(DOMAIN inbound_peer_requests) <= 2 /\ Cardinality(reserved_interaction_streams) <= 2 /\ Cardinality(attached_interaction_streams) <= 2 /\ Cardinality(direct_peer_endpoints) <= 2 /\ Cardinality(mob_overlay_peer_endpoints) <= 2

Spec == Init /\ [][Next]_vars

THEOREM Spec => []fence_requires_bound_runtime
THEOREM Spec => []running_has_current_run
THEOREM Spec => []current_run_only_while_running_or_retired
THEOREM Spec => []realtime_binding_epoch_consistency
THEOREM Spec => []peer_ingress_owner_consistency
THEOREM Spec => []supervisor_binding_consistency

=============================================================================
