---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for meerkat_mob_seam.

CONSTANTS AgentIdentityValues, AgentRuntimeIdValues, BooleanValues, FenceTokenValues, GenerationValues, InputIdValues, McpServerIdValues, McpServerStateValues, MobMemberStateValues, MobTaskValues, NatValues, RealtimeBindingStateValues, RunIdValues, SessionIdValues, SessionLlmCapabilitySurfaceStatusValues, SessionLlmCapabilitySurfaceValues, SessionLlmIdentityValues, SessionToolVisibilityDeltaValues, SessionToolVisibilityStateValues, SetOfAgentRuntimeIdValues, SetOfStringValues, SetOfTaskIdValues, SetOfWiringEdgeValues, StringValues, TaskIdValues, TaskStatusValues, ToolFilterValues, ToolVisibilityWitnessValues, WiringEdgeValues, WorkIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionAgentRuntimeIdValues == {None} \cup {Some(x) : x \in AgentRuntimeIdValues}
OptionFenceTokenValues == {None} \cup {Some(x) : x \in FenceTokenValues}
OptionRunIdValues == {None} \cup {Some(x) : x \in RunIdValues}
OptionSessionIdValues == {None} \cup {Some(x) : x \in SessionIdValues}
OptionSessionLlmCapabilitySurfaceValues == {None} \cup {Some(x) : x \in SessionLlmCapabilitySurfaceValues}
OptionStringValues == {None} \cup {Some(x) : x \in StringValues}
OptionU64Values == {None} \cup {Some(x) : x \in NatValues}
MapAgentIdentityAgentRuntimeIdValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in AgentIdentityValues, v \in AgentRuntimeIdValues }
MapAgentRuntimeIdFenceTokenValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in AgentRuntimeIdValues, v \in FenceTokenValues }
MapAgentRuntimeIdMobMemberStateValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in AgentRuntimeIdValues, v \in MobMemberStateValues }
MapMcpServerIdMcpServerStateValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in McpServerIdValues, v \in McpServerStateValues }
MapStringToolVisibilityWitnessValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in ToolVisibilityWitnessValues }
MapTaskIdMobTaskValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in TaskIdValues, v \in MobTaskValues }

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
MapRemove(map, key) == [x \in DOMAIN map \ {key} |-> map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN Tail(seq) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))
AppendIfMissing(seq, value) == IF value \in SeqElements(seq) THEN seq ELSE Append(seq, value)
Machines == {
    <<"meerkat", "MeerkatMachine", "meerkat_kernel">>,
    <<"mob", "MobMachine", "mob_kernel">>
}

RouteNames == {
    "binding_request_reaches_meerkat",
    "work_request_reaches_meerkat",
    "retire_request_reaches_meerkat",
    "destroy_request_reaches_meerkat",
    "runtime_bound_reaches_mob",
    "runtime_retired_reaches_mob",
    "runtime_destroyed_reaches_mob"
}

Actors == {
    "meerkat_kernel",
    "mob_kernel"
}

ActorPriorities == {
}

SchedulerRules == {
}

ActorOfMachine(machine_id) ==
    CASE machine_id = "meerkat" -> "meerkat_kernel"
      [] machine_id = "mob" -> "mob_kernel"
      [] OTHER -> "unknown_actor"

RouteSource(route_name) ==
    CASE route_name = "binding_request_reaches_meerkat" -> "mob"
      [] route_name = "work_request_reaches_meerkat" -> "mob"
      [] route_name = "retire_request_reaches_meerkat" -> "mob"
      [] route_name = "destroy_request_reaches_meerkat" -> "mob"
      [] route_name = "runtime_bound_reaches_mob" -> "meerkat"
      [] route_name = "runtime_retired_reaches_mob" -> "meerkat"
      [] route_name = "runtime_destroyed_reaches_mob" -> "meerkat"
      [] OTHER -> "unknown_machine"

RouteEffect(route_name) ==
    CASE route_name = "binding_request_reaches_meerkat" -> "RequestRuntimeBinding"
      [] route_name = "work_request_reaches_meerkat" -> "RequestRuntimeIngress"
      [] route_name = "retire_request_reaches_meerkat" -> "RequestRuntimeRetire"
      [] route_name = "destroy_request_reaches_meerkat" -> "RequestRuntimeDestroy"
      [] route_name = "runtime_bound_reaches_mob" -> "RuntimeBound"
      [] route_name = "runtime_retired_reaches_mob" -> "RuntimeRetired"
      [] route_name = "runtime_destroyed_reaches_mob" -> "RuntimeDestroyed"
      [] OTHER -> "unknown_effect"

RouteTargetMachine(route_name) ==
    CASE route_name = "binding_request_reaches_meerkat" -> "meerkat"
      [] route_name = "work_request_reaches_meerkat" -> "meerkat"
      [] route_name = "retire_request_reaches_meerkat" -> "meerkat"
      [] route_name = "destroy_request_reaches_meerkat" -> "meerkat"
      [] route_name = "runtime_bound_reaches_mob" -> "mob"
      [] route_name = "runtime_retired_reaches_mob" -> "mob"
      [] route_name = "runtime_destroyed_reaches_mob" -> "mob"
      [] OTHER -> "unknown_machine"

RouteTargetInput(route_name) ==
    CASE route_name = "binding_request_reaches_meerkat" -> "PrepareBindings"
      [] route_name = "work_request_reaches_meerkat" -> "Ingest"
      [] route_name = "retire_request_reaches_meerkat" -> "Retire"
      [] route_name = "destroy_request_reaches_meerkat" -> "Destroy"
      [] route_name = "runtime_bound_reaches_mob" -> "ObserveRuntimeReady"
      [] route_name = "runtime_retired_reaches_mob" -> "ObserveRuntimeRetired"
      [] route_name = "runtime_destroyed_reaches_mob" -> "ObserveRuntimeDestroyed"
      [] OTHER -> "unknown_input"

RouteTargetKind(route_name) ==
    CASE route_name = "binding_request_reaches_meerkat" -> "Input"
      [] route_name = "work_request_reaches_meerkat" -> "Input"
      [] route_name = "retire_request_reaches_meerkat" -> "Input"
      [] route_name = "destroy_request_reaches_meerkat" -> "Input"
      [] route_name = "runtime_bound_reaches_mob" -> "Signal"
      [] route_name = "runtime_retired_reaches_mob" -> "Signal"
      [] route_name = "runtime_destroyed_reaches_mob" -> "Signal"
      [] OTHER -> "Unknown"

RouteDeliveryKind(route_name) ==
    CASE route_name = "binding_request_reaches_meerkat" -> "Immediate"
      [] route_name = "work_request_reaches_meerkat" -> "Immediate"
      [] route_name = "retire_request_reaches_meerkat" -> "Immediate"
      [] route_name = "destroy_request_reaches_meerkat" -> "Immediate"
      [] route_name = "runtime_bound_reaches_mob" -> "Immediate"
      [] route_name = "runtime_retired_reaches_mob" -> "Immediate"
      [] route_name = "runtime_destroyed_reaches_mob" -> "Immediate"
      [] OTHER -> "Unknown"

RouteTargetActor(route_name) == ActorOfMachine(RouteTargetMachine(route_name))

VARIABLES meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

RoutePackets == SeqElements(pending_routes) \cup delivered_routes
PendingActors == {ActorOfMachine(packet.machine) : packet \in SeqElements(pending_inputs)}
HigherPriorityReady(actor) == \E priority \in ActorPriorities : /\ priority[2] = actor /\ priority[1] \in PendingActors

BaseInit ==
    /\ meerkat_phase = "Initializing"
    /\ meerkat_session_id = None
    /\ meerkat_active_runtime_id = None
    /\ meerkat_active_fence_token = None
    /\ meerkat_current_run_id = None
    /\ meerkat_pre_run_phase = None
    /\ meerkat_silent_intent_overrides = {}
    /\ meerkat_realtime_intent_present = FALSE
    /\ meerkat_realtime_binding_state = "Unbound"
    /\ meerkat_realtime_binding_authority_epoch = None
    /\ meerkat_realtime_reattach_required = FALSE
    /\ meerkat_realtime_next_authority_epoch = 1
    /\ meerkat_live_topology_phase = "Idle"
    /\ meerkat_mcp_server_states = [x \in {} |-> None]
    /\ mob_phase = "Running"
    /\ mob_live_runtime_ids = {}
    /\ mob_externally_addressable_runtime_ids = {}
    /\ mob_runtime_fence_tokens = [x \in {} |-> None]
    /\ mob_active_run_count = 0
    /\ mob_pending_spawn_count = 0
    /\ mob_coordinator_bound = TRUE
    /\ mob_member_state_markers = [x \in {} |-> None]
    /\ mob_wiring_edges = {}
    /\ mob_identity_to_runtime = [x \in {} |-> None]
    /\ mob_tasks = [x \in {} |-> None]
    /\ mob_in_progress_task_ids = {}
    /\ mob_completed_task_ids = {}
    /\ model_step_count = 0
    /\ pending_routes = <<>>
    /\ delivered_routes = {}
    /\ emitted_effects = {}
    /\ observed_transitions = {}

Init ==
    /\ BaseInit
    /\ pending_inputs = <<>>
    /\ observed_inputs = {}
    /\ witness_current_script_input = None
    /\ witness_remaining_script_inputs = <<>>

WitnessInit_basic_round_trip ==
    /\ BaseInit
    /\ pending_inputs = <<>>
    /\ observed_inputs = {}
    /\ witness_current_script_input = None
    /\ witness_remaining_script_inputs = <<>>

WitnessInit_retire_runtime_path ==
    /\ BaseInit
    /\ pending_inputs = <<>>
    /\ observed_inputs = {}
    /\ witness_current_script_input = None
    /\ witness_remaining_script_inputs = <<>>

WitnessInit_destroy_runtime_path ==
    /\ BaseInit
    /\ pending_inputs = <<>>
    /\ observed_inputs = {}
    /\ witness_current_script_input = None
    /\ witness_remaining_script_inputs = <<>>

meerkat_Initialize ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Initialize"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Initializing"
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "Initialize", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RegisterSessionIdle(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RegisterSession"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ meerkat_phase' = "Idle"
       /\ meerkat_session_id' = Some(packet.payload.session_id)
       /\ UNCHANGED << meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RegisterSessionIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RegisterSessionAttached(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RegisterSession"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ meerkat_phase' = "Attached"
       /\ meerkat_session_id' = Some(packet.payload.session_id)
       /\ UNCHANGED << meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RegisterSessionAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RegisterSessionRunning(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RegisterSession"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ meerkat_phase' = "Running"
       /\ meerkat_session_id' = Some(packet.payload.session_id)
       /\ UNCHANGED << meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RegisterSessionRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RegisterSessionRetired(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RegisterSession"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ meerkat_phase' = "Retired"
       /\ meerkat_session_id' = Some(packet.payload.session_id)
       /\ UNCHANGED << meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RegisterSessionRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RegisterSessionStopped(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RegisterSession"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_session_id' = Some(packet.payload.session_id)
       /\ UNCHANGED << meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RegisterSessionStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_UnregisterSessionIdle(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "UnregisterSession"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id = Some(packet.payload.session_id))
       /\ meerkat_phase' = "Idle"
       /\ meerkat_session_id' = None
       /\ meerkat_active_runtime_id' = None
       /\ meerkat_active_fence_token' = None
       /\ meerkat_current_run_id' = None
       /\ meerkat_pre_run_phase' = None
       /\ UNCHANGED << meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "UnregisterSessionIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_UnregisterSessionAttached(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "UnregisterSession"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id = Some(packet.payload.session_id))
       /\ meerkat_phase' = "Idle"
       /\ meerkat_session_id' = None
       /\ meerkat_active_runtime_id' = None
       /\ meerkat_active_fence_token' = None
       /\ meerkat_current_run_id' = None
       /\ meerkat_pre_run_phase' = None
       /\ UNCHANGED << meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "UnregisterSessionAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_UnregisterSessionRunning(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "UnregisterSession"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id = Some(packet.payload.session_id))
       /\ meerkat_phase' = "Idle"
       /\ meerkat_session_id' = None
       /\ meerkat_active_runtime_id' = None
       /\ meerkat_active_fence_token' = None
       /\ meerkat_current_run_id' = None
       /\ meerkat_pre_run_phase' = None
       /\ UNCHANGED << meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "UnregisterSessionRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_UnregisterSessionRetired(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "UnregisterSession"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id = Some(packet.payload.session_id))
       /\ meerkat_phase' = "Idle"
       /\ meerkat_session_id' = None
       /\ meerkat_active_runtime_id' = None
       /\ meerkat_active_fence_token' = None
       /\ meerkat_current_run_id' = None
       /\ meerkat_pre_run_phase' = None
       /\ UNCHANGED << meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "UnregisterSessionRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_UnregisterSessionStopped(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "UnregisterSession"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id = Some(packet.payload.session_id))
       /\ meerkat_phase' = "Idle"
       /\ meerkat_session_id' = None
       /\ meerkat_active_runtime_id' = None
       /\ meerkat_active_fence_token' = None
       /\ meerkat_current_run_id' = None
       /\ meerkat_pre_run_phase' = None
       /\ UNCHANGED << meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "UnregisterSessionStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ReconfigureSessionLlmIdentityAttached(arg_previous_identity, arg_previous_visibility_state, arg_previous_capability_surface, arg_previous_capability_surface_status, arg_target_identity, arg_target_capability_surface, arg_next_visibility_state, arg_next_capability_base_filter, arg_next_active_visibility_revision, arg_tool_visibility_delta) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ReconfigureSessionLlmIdentity"
       /\ packet.payload.previous_identity = arg_previous_identity
       /\ packet.payload.previous_visibility_state = arg_previous_visibility_state
       /\ packet.payload.previous_capability_surface = arg_previous_capability_surface
       /\ packet.payload.previous_capability_surface_status = arg_previous_capability_surface_status
       /\ packet.payload.target_identity = arg_target_identity
       /\ packet.payload.target_capability_surface = arg_target_capability_surface
       /\ packet.payload.next_visibility_state = arg_next_visibility_state
       /\ packet.payload.next_capability_base_filter = arg_next_capability_base_filter
       /\ packet.payload.next_active_visibility_revision = arg_next_active_visibility_revision
       /\ packet.payload.tool_visibility_delta = arg_tool_visibility_delta
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ (meerkat_active_runtime_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ReconfigureSessionLlmIdentityAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ReconfigureSessionLlmIdentityRunning(arg_previous_identity, arg_previous_visibility_state, arg_previous_capability_surface, arg_previous_capability_surface_status, arg_target_identity, arg_target_capability_surface, arg_next_visibility_state, arg_next_capability_base_filter, arg_next_active_visibility_revision, arg_tool_visibility_delta) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ReconfigureSessionLlmIdentity"
       /\ packet.payload.previous_identity = arg_previous_identity
       /\ packet.payload.previous_visibility_state = arg_previous_visibility_state
       /\ packet.payload.previous_capability_surface = arg_previous_capability_surface
       /\ packet.payload.previous_capability_surface_status = arg_previous_capability_surface_status
       /\ packet.payload.target_identity = arg_target_identity
       /\ packet.payload.target_capability_surface = arg_target_capability_surface
       /\ packet.payload.next_visibility_state = arg_next_visibility_state
       /\ packet.payload.next_capability_base_filter = arg_next_capability_base_filter
       /\ packet.payload.next_active_visibility_revision = arg_next_active_visibility_revision
       /\ packet.payload.tool_visibility_delta = arg_tool_visibility_delta
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ (meerkat_active_runtime_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ReconfigureSessionLlmIdentityRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StagePersistentFilterIdle(arg_filter, arg_witnesses) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StagePersistentFilter"
       /\ packet.payload.filter = arg_filter
       /\ packet.payload.witnesses = arg_witnesses
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StagePersistentFilterIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StagePersistentFilterAttached(arg_filter, arg_witnesses) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StagePersistentFilter"
       /\ packet.payload.filter = arg_filter
       /\ packet.payload.witnesses = arg_witnesses
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StagePersistentFilterAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StagePersistentFilterRunning(arg_filter, arg_witnesses) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StagePersistentFilter"
       /\ packet.payload.filter = arg_filter
       /\ packet.payload.witnesses = arg_witnesses
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StagePersistentFilterRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StagePersistentFilterRetired(arg_filter, arg_witnesses) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StagePersistentFilter"
       /\ packet.payload.filter = arg_filter
       /\ packet.payload.witnesses = arg_witnesses
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Retired"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StagePersistentFilterRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StagePersistentFilterStopped(arg_filter, arg_witnesses) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StagePersistentFilter"
       /\ packet.payload.filter = arg_filter
       /\ packet.payload.witnesses = arg_witnesses
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Stopped"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StagePersistentFilterStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RequestDeferredToolsIdle(arg_names, arg_witnesses) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RequestDeferredTools"
       /\ packet.payload.names = arg_names
       /\ packet.payload.witnesses = arg_witnesses
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RequestDeferredToolsIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RequestDeferredToolsAttached(arg_names, arg_witnesses) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RequestDeferredTools"
       /\ packet.payload.names = arg_names
       /\ packet.payload.witnesses = arg_witnesses
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RequestDeferredToolsAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RequestDeferredToolsRunning(arg_names, arg_witnesses) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RequestDeferredTools"
       /\ packet.payload.names = arg_names
       /\ packet.payload.witnesses = arg_witnesses
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RequestDeferredToolsRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RequestDeferredToolsRetired(arg_names, arg_witnesses) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RequestDeferredTools"
       /\ packet.payload.names = arg_names
       /\ packet.payload.witnesses = arg_witnesses
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Retired"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RequestDeferredToolsRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RequestDeferredToolsStopped(arg_names, arg_witnesses) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RequestDeferredTools"
       /\ packet.payload.names = arg_names
       /\ packet.payload.witnesses = arg_witnesses
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Stopped"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RequestDeferredToolsStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PrepareBindingsInitializing(arg_agent_runtime_id, arg_fence_token, arg_generation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PrepareBindings"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.generation = arg_generation
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Initializing"
       /\ meerkat_phase' = "Initializing"
       /\ meerkat_active_runtime_id' = Some(packet.payload.agent_runtime_id)
       /\ meerkat_active_fence_token' = Some(packet.payload.fence_token)
       /\ UNCHANGED << meerkat_session_id, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "mob", variant |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeBound", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeBound", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", effect |-> "RuntimeBound", target_machine |-> "mob", target_input |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], actor |-> "mob_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "PrepareBindingsInitializing"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeBound", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "PrepareBindingsInitializing"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PrepareBindingsInitializing", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Initializing"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PrepareBindingsIdle(arg_agent_runtime_id, arg_fence_token, arg_generation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PrepareBindings"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.generation = arg_generation
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ meerkat_phase' = "Attached"
       /\ meerkat_active_runtime_id' = Some(packet.payload.agent_runtime_id)
       /\ meerkat_active_fence_token' = Some(packet.payload.fence_token)
       /\ UNCHANGED << meerkat_session_id, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "mob", variant |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeBound", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeBound", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", effect |-> "RuntimeBound", target_machine |-> "mob", target_input |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], actor |-> "mob_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "PrepareBindingsIdle"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeBound", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "PrepareBindingsIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PrepareBindingsIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PrepareBindingsAttached(arg_agent_runtime_id, arg_fence_token, arg_generation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PrepareBindings"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.generation = arg_generation
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ meerkat_phase' = "Attached"
       /\ meerkat_active_runtime_id' = Some(packet.payload.agent_runtime_id)
       /\ meerkat_active_fence_token' = Some(packet.payload.fence_token)
       /\ UNCHANGED << meerkat_session_id, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "mob", variant |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeBound", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeBound", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", effect |-> "RuntimeBound", target_machine |-> "mob", target_input |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], actor |-> "mob_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "PrepareBindingsAttached"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeBound", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "PrepareBindingsAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PrepareBindingsAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PrepareBindingsRunning(arg_agent_runtime_id, arg_fence_token, arg_generation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PrepareBindings"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.generation = arg_generation
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ meerkat_phase' = "Running"
       /\ meerkat_active_runtime_id' = Some(packet.payload.agent_runtime_id)
       /\ meerkat_active_fence_token' = Some(packet.payload.fence_token)
       /\ UNCHANGED << meerkat_session_id, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "mob", variant |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeBound", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeBound", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", effect |-> "RuntimeBound", target_machine |-> "mob", target_input |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], actor |-> "mob_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "PrepareBindingsRunning"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeBound", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "PrepareBindingsRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PrepareBindingsRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PrepareBindingsRetired(arg_agent_runtime_id, arg_fence_token, arg_generation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PrepareBindings"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.generation = arg_generation
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ meerkat_phase' = "Retired"
       /\ meerkat_active_runtime_id' = Some(packet.payload.agent_runtime_id)
       /\ meerkat_active_fence_token' = Some(packet.payload.fence_token)
       /\ UNCHANGED << meerkat_session_id, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "mob", variant |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeBound", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeBound", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", effect |-> "RuntimeBound", target_machine |-> "mob", target_input |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], actor |-> "mob_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "PrepareBindingsRetired"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeBound", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "PrepareBindingsRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PrepareBindingsRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PrepareBindingsStopped(arg_agent_runtime_id, arg_fence_token, arg_generation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PrepareBindings"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.generation = arg_generation
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_active_runtime_id' = Some(packet.payload.agent_runtime_id)
       /\ meerkat_active_fence_token' = Some(packet.payload.fence_token)
       /\ UNCHANGED << meerkat_session_id, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "mob", variant |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeBound", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeBound", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", effect |-> "RuntimeBound", target_machine |-> "mob", target_input |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], actor |-> "mob_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "PrepareBindingsStopped"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeBound", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "PrepareBindingsStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PrepareBindingsStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_SetPeerIngressContextIdle(arg_keep_alive) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "SetPeerIngressContext"
       /\ packet.payload.keep_alive = arg_keep_alive
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "SetPeerIngressContextIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_SetPeerIngressContextAttached(arg_keep_alive) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "SetPeerIngressContext"
       /\ packet.payload.keep_alive = arg_keep_alive
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "SetPeerIngressContextAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_SetPeerIngressContextRunning(arg_keep_alive) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "SetPeerIngressContext"
       /\ packet.payload.keep_alive = arg_keep_alive
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "SetPeerIngressContextRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_SetPeerIngressContextRetired(arg_keep_alive) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "SetPeerIngressContext"
       /\ packet.payload.keep_alive = arg_keep_alive
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Retired"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "SetPeerIngressContextRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_SetPeerIngressContextStopped(arg_keep_alive) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "SetPeerIngressContext"
       /\ packet.payload.keep_alive = arg_keep_alive
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Stopped"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "SetPeerIngressContextStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_NotifyDrainExitedIdle(arg_reason) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "NotifyDrainExited"
       /\ packet.payload.reason = arg_reason
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeNotice", payload |-> [detail |-> "drain exited", kind |-> "drain"], effect_id |-> (model_step_count + 1), source_transition |-> "NotifyDrainExitedIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "NotifyDrainExitedIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_NotifyDrainExitedAttached(arg_reason) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "NotifyDrainExited"
       /\ packet.payload.reason = arg_reason
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeNotice", payload |-> [detail |-> "drain exited", kind |-> "drain"], effect_id |-> (model_step_count + 1), source_transition |-> "NotifyDrainExitedAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "NotifyDrainExitedAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_NotifyDrainExitedRunning(arg_reason) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "NotifyDrainExited"
       /\ packet.payload.reason = arg_reason
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeNotice", payload |-> [detail |-> "drain exited", kind |-> "drain"], effect_id |-> (model_step_count + 1), source_transition |-> "NotifyDrainExitedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "NotifyDrainExitedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_NotifyDrainExitedRetired(arg_reason) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "NotifyDrainExited"
       /\ packet.payload.reason = arg_reason
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Retired"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeNotice", payload |-> [detail |-> "drain exited", kind |-> "drain"], effect_id |-> (model_step_count + 1), source_transition |-> "NotifyDrainExitedRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "NotifyDrainExitedRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_NotifyDrainExitedStopped(arg_reason) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "NotifyDrainExited"
       /\ packet.payload.reason = arg_reason
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Stopped"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeNotice", payload |-> [detail |-> "drain exited", kind |-> "drain"], effect_id |-> (model_step_count + 1), source_transition |-> "NotifyDrainExitedStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "NotifyDrainExitedStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_InterruptCurrentRunAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "InterruptCurrentRun"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "WakeInterrupt", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "InterruptCurrentRunAttached"], [machine |-> "meerkat", variant |-> "RequestCancellationAtBoundary", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "InterruptCurrentRunAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "InterruptCurrentRunAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_InterruptCurrentRun ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "InterruptCurrentRun"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "WakeInterrupt", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "InterruptCurrentRun"], [machine |-> "meerkat", variant |-> "RequestCancellationAtBoundary", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "InterruptCurrentRun"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "InterruptCurrentRun", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CancelAfterBoundaryAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "CancelAfterBoundary"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RequestCancellationAtBoundary", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CancelAfterBoundaryAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CancelAfterBoundaryAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CancelAfterBoundary ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "CancelAfterBoundary"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RequestCancellationAtBoundary", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CancelAfterBoundary"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CancelAfterBoundary", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_BoundaryAppliedPublish(arg_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "BoundaryApplied"
       /\ packet.payload.revision = arg_revision
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "CommittedVisibleSetPublished", payload |-> [revision |-> packet.payload.revision], effect_id |-> (model_step_count + 1), source_transition |-> "BoundaryAppliedPublish"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "BoundaryAppliedPublish", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PublishCommittedVisibleSetIdle(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PublishCommittedVisibleSet"
       /\ packet.payload.active_filter = arg_active_filter
       /\ packet.payload.staged_filter = arg_staged_filter
       /\ packet.payload.active_requested_deferred_names = arg_active_requested_deferred_names
       /\ packet.payload.staged_requested_deferred_names = arg_staged_requested_deferred_names
       /\ packet.payload.active_visibility_revision = arg_active_visibility_revision
       /\ packet.payload.staged_visibility_revision = arg_staged_visibility_revision
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ (packet.payload.active_visibility_revision >= packet.payload.staged_visibility_revision)
       /\ ((packet.payload.active_visibility_revision # packet.payload.staged_visibility_revision) \/ ((packet.payload.active_filter = packet.payload.staged_filter) /\ (packet.payload.active_requested_deferred_names = packet.payload.staged_requested_deferred_names)))
       /\ (\A requested_name \in packet.payload.active_requested_deferred_names : (requested_name \in packet.payload.staged_requested_deferred_names))
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "CommittedVisibleSetPublished", payload |-> [revision |-> packet.payload.active_visibility_revision], effect_id |-> (model_step_count + 1), source_transition |-> "PublishCommittedVisibleSetIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PublishCommittedVisibleSetIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PublishCommittedVisibleSetAttached(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PublishCommittedVisibleSet"
       /\ packet.payload.active_filter = arg_active_filter
       /\ packet.payload.staged_filter = arg_staged_filter
       /\ packet.payload.active_requested_deferred_names = arg_active_requested_deferred_names
       /\ packet.payload.staged_requested_deferred_names = arg_staged_requested_deferred_names
       /\ packet.payload.active_visibility_revision = arg_active_visibility_revision
       /\ packet.payload.staged_visibility_revision = arg_staged_visibility_revision
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ (packet.payload.active_visibility_revision >= packet.payload.staged_visibility_revision)
       /\ ((packet.payload.active_visibility_revision # packet.payload.staged_visibility_revision) \/ ((packet.payload.active_filter = packet.payload.staged_filter) /\ (packet.payload.active_requested_deferred_names = packet.payload.staged_requested_deferred_names)))
       /\ (\A requested_name \in packet.payload.active_requested_deferred_names : (requested_name \in packet.payload.staged_requested_deferred_names))
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "CommittedVisibleSetPublished", payload |-> [revision |-> packet.payload.active_visibility_revision], effect_id |-> (model_step_count + 1), source_transition |-> "PublishCommittedVisibleSetAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PublishCommittedVisibleSetAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PublishCommittedVisibleSetRunning(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PublishCommittedVisibleSet"
       /\ packet.payload.active_filter = arg_active_filter
       /\ packet.payload.staged_filter = arg_staged_filter
       /\ packet.payload.active_requested_deferred_names = arg_active_requested_deferred_names
       /\ packet.payload.staged_requested_deferred_names = arg_staged_requested_deferred_names
       /\ packet.payload.active_visibility_revision = arg_active_visibility_revision
       /\ packet.payload.staged_visibility_revision = arg_staged_visibility_revision
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ (packet.payload.active_visibility_revision >= packet.payload.staged_visibility_revision)
       /\ ((packet.payload.active_visibility_revision # packet.payload.staged_visibility_revision) \/ ((packet.payload.active_filter = packet.payload.staged_filter) /\ (packet.payload.active_requested_deferred_names = packet.payload.staged_requested_deferred_names)))
       /\ (\A requested_name \in packet.payload.active_requested_deferred_names : (requested_name \in packet.payload.staged_requested_deferred_names))
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "CommittedVisibleSetPublished", payload |-> [revision |-> packet.payload.active_visibility_revision], effect_id |-> (model_step_count + 1), source_transition |-> "PublishCommittedVisibleSetRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PublishCommittedVisibleSetRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PublishCommittedVisibleSetRetired(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PublishCommittedVisibleSet"
       /\ packet.payload.active_filter = arg_active_filter
       /\ packet.payload.staged_filter = arg_staged_filter
       /\ packet.payload.active_requested_deferred_names = arg_active_requested_deferred_names
       /\ packet.payload.staged_requested_deferred_names = arg_staged_requested_deferred_names
       /\ packet.payload.active_visibility_revision = arg_active_visibility_revision
       /\ packet.payload.staged_visibility_revision = arg_staged_visibility_revision
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ (packet.payload.active_visibility_revision >= packet.payload.staged_visibility_revision)
       /\ ((packet.payload.active_visibility_revision # packet.payload.staged_visibility_revision) \/ ((packet.payload.active_filter = packet.payload.staged_filter) /\ (packet.payload.active_requested_deferred_names = packet.payload.staged_requested_deferred_names)))
       /\ (\A requested_name \in packet.payload.active_requested_deferred_names : (requested_name \in packet.payload.staged_requested_deferred_names))
       /\ meerkat_phase' = "Retired"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "CommittedVisibleSetPublished", payload |-> [revision |-> packet.payload.active_visibility_revision], effect_id |-> (model_step_count + 1), source_transition |-> "PublishCommittedVisibleSetRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PublishCommittedVisibleSetRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PublishCommittedVisibleSetStopped(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PublishCommittedVisibleSet"
       /\ packet.payload.active_filter = arg_active_filter
       /\ packet.payload.staged_filter = arg_staged_filter
       /\ packet.payload.active_requested_deferred_names = arg_active_requested_deferred_names
       /\ packet.payload.staged_requested_deferred_names = arg_staged_requested_deferred_names
       /\ packet.payload.active_visibility_revision = arg_active_visibility_revision
       /\ packet.payload.staged_visibility_revision = arg_staged_visibility_revision
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ (packet.payload.active_visibility_revision >= packet.payload.staged_visibility_revision)
       /\ ((packet.payload.active_visibility_revision # packet.payload.staged_visibility_revision) \/ ((packet.payload.active_filter = packet.payload.staged_filter) /\ (packet.payload.active_requested_deferred_names = packet.payload.staged_requested_deferred_names)))
       /\ (\A requested_name \in packet.payload.active_requested_deferred_names : (requested_name \in packet.payload.staged_requested_deferred_names))
       /\ meerkat_phase' = "Stopped"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "CommittedVisibleSetPublished", payload |-> [revision |-> packet.payload.active_visibility_revision], effect_id |-> (model_step_count + 1), source_transition |-> "PublishCommittedVisibleSetStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PublishCommittedVisibleSetStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RetireRequestedFromIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Retire"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle" \/ meerkat_phase = "Attached" \/ meerkat_phase = "Running"
       /\ meerkat_phase' = "Retired"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "mob", variant |-> "ObserveRuntimeRetired", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_retired_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeRetired", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "ObserveRuntimeRetired", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_retired_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeRetired", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "runtime_retired_reaches_mob", source_machine |-> "meerkat", effect |-> "RuntimeRetired", target_machine |-> "mob", target_input |-> "ObserveRuntimeRetired", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None)], actor |-> "mob_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "RetireRequestedFromIdle"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeRetired", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "RetireRequestedFromIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RetireRequestedFromIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_Reset ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Reset"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Initializing" \/ meerkat_phase = "Idle" \/ meerkat_phase = "Attached" \/ meerkat_phase = "Retired"
       /\ meerkat_phase' = "Idle"
       /\ meerkat_active_fence_token' = None
       /\ meerkat_current_run_id' = None
       /\ meerkat_pre_run_phase' = None
       /\ meerkat_silent_intent_overrides' = {}
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeNotice", payload |-> [detail |-> "runtime reset", kind |-> "reset"], effect_id |-> (model_step_count + 1), source_transition |-> "Reset"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "Reset", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StopRuntimeExecutorUnbound ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StopRuntimeExecutor"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Initializing" \/ meerkat_phase = "Idle" \/ meerkat_phase = "Retired"
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_current_run_id' = None
       /\ meerkat_pre_run_phase' = None
       /\ meerkat_silent_intent_overrides' = {}
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeNotice", payload |-> [detail |-> "runtime executor stopped", kind |-> "stop"], effect_id |-> (model_step_count + 1), source_transition |-> "StopRuntimeExecutorUnbound"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StopRuntimeExecutorUnbound", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StopRuntimeExecutorAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StopRuntimeExecutor"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ meerkat_phase' = "Attached"
       /\ meerkat_silent_intent_overrides' = {}
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeNotice", payload |-> [detail |-> "runtime executor stopped", kind |-> "stop"], effect_id |-> (model_step_count + 1), source_transition |-> "StopRuntimeExecutorAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StopRuntimeExecutorAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StopRuntimeExecutorRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StopRuntimeExecutor"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ meerkat_phase' = "Running"
       /\ meerkat_silent_intent_overrides' = {}
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeNotice", payload |-> [detail |-> "runtime executor stopped", kind |-> "stop"], effect_id |-> (model_step_count + 1), source_transition |-> "StopRuntimeExecutorRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StopRuntimeExecutorRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_Destroy ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Destroy"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Initializing" \/ meerkat_phase = "Idle" \/ meerkat_phase = "Attached" \/ meerkat_phase = "Running" \/ meerkat_phase = "Retired" \/ meerkat_phase = "Stopped"
       /\ (meerkat_active_runtime_id # None)
       /\ meerkat_phase' = "Destroyed"
       /\ meerkat_current_run_id' = None
       /\ meerkat_pre_run_phase' = None
       /\ meerkat_silent_intent_overrides' = {}
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "mob", variant |-> "ObserveRuntimeDestroyed", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_destroyed_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeDestroyed", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "ObserveRuntimeDestroyed", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_destroyed_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeDestroyed", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "runtime_destroyed_reaches_mob", source_machine |-> "meerkat", effect |-> "RuntimeDestroyed", target_machine |-> "mob", target_input |-> "ObserveRuntimeDestroyed", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None)], actor |-> "mob_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "Destroy"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeDestroyed", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "Destroy"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "Destroy", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


meerkat_EnsureSessionWithExecutorIdle(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "EnsureSessionWithExecutor"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "EnsureSessionWithExecutorIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_EnsureSessionWithExecutorAttached(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "EnsureSessionWithExecutor"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "EnsureSessionWithExecutorAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_EnsureSessionWithExecutorRunning(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "EnsureSessionWithExecutor"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "EnsureSessionWithExecutorRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_EnsureSessionWithExecutorRetired(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "EnsureSessionWithExecutor"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ meerkat_phase' = "Retired"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "EnsureSessionWithExecutorRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_EnsureSessionWithExecutorStopped(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "EnsureSessionWithExecutor"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ meerkat_phase' = "Stopped"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "EnsureSessionWithExecutorStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_SetSilentIntentsIdle(arg_session_id, arg_intents) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "SetSilentIntents"
       /\ packet.payload.session_id = arg_session_id
       /\ packet.payload.intents = arg_intents
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ meerkat_silent_intent_overrides' = packet.payload.intents
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "SetSilentIntentsIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_SetSilentIntentsAttached(arg_session_id, arg_intents) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "SetSilentIntents"
       /\ packet.payload.session_id = arg_session_id
       /\ packet.payload.intents = arg_intents
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ meerkat_silent_intent_overrides' = packet.payload.intents
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "SetSilentIntentsAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_SetSilentIntentsRunning(arg_session_id, arg_intents) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "SetSilentIntents"
       /\ packet.payload.session_id = arg_session_id
       /\ packet.payload.intents = arg_intents
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ meerkat_silent_intent_overrides' = packet.payload.intents
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "SetSilentIntentsRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_SetSilentIntentsRetired(arg_session_id, arg_intents) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "SetSilentIntents"
       /\ packet.payload.session_id = arg_session_id
       /\ packet.payload.intents = arg_intents
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Retired"
       /\ meerkat_silent_intent_overrides' = packet.payload.intents
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "SetSilentIntentsRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_SetSilentIntentsStopped(arg_session_id, arg_intents) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "SetSilentIntents"
       /\ packet.payload.session_id = arg_session_id
       /\ packet.payload.intents = arg_intents
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Stopped"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "SetSilentIntentsStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AbortIdle(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Abort"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AbortIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AbortAttached(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Abort"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AbortAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AbortRunning(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Abort"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AbortRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AbortRetired(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Abort"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Retired"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AbortRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AbortStopped(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Abort"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Stopped"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AbortStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_WaitIdle(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Wait"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "WaitIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_WaitAttached(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Wait"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "WaitAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_WaitRunning(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Wait"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "WaitRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_WaitRetired(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Wait"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Retired"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "WaitRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_WaitStopped(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Wait"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Stopped"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "WaitStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AbortAllIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AbortAll"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AbortAllIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AbortAllAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AbortAll"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AbortAllAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AbortAllRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AbortAll"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AbortAllRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AbortAllRetired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AbortAll"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ meerkat_phase' = "Retired"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AbortAllRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AbortAllStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AbortAll"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ meerkat_phase' = "Stopped"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AbortAllStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_EnsureDrainRunningAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "EnsureDrainRunning"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "SpawnDrainTask", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "EnsureDrainRunningAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "EnsureDrainRunningAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_EnsureDrainRunningRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "EnsureDrainRunning"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "SpawnDrainTask", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "EnsureDrainRunningRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "EnsureDrainRunningRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_IngestIdle(arg_runtime_id, arg_work_id, arg_origin) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Ingest"
       /\ packet.payload.runtime_id = arg_runtime_id
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.origin = arg_origin
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "ResolveAdmission", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "IngestIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "IngestIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_IngestAttached(arg_runtime_id, arg_work_id, arg_origin) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Ingest"
       /\ packet.payload.runtime_id = arg_runtime_id
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.origin = arg_origin
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "ResolveAdmission", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "IngestAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "IngestAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_IngestRunning(arg_runtime_id, arg_work_id, arg_origin) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Ingest"
       /\ packet.payload.runtime_id = arg_runtime_id
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.origin = arg_origin
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "ResolveAdmission", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "IngestRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "IngestRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PublishEventIdle(arg_kind) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PublishEvent"
       /\ packet.payload.kind = arg_kind
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "IngressNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PublishEventIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PublishEventIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PublishEventAttached(arg_kind) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PublishEvent"
       /\ packet.payload.kind = arg_kind
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "IngressNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PublishEventAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PublishEventAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PublishEventRunning(arg_kind) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PublishEvent"
       /\ packet.payload.kind = arg_kind
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "IngressNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PublishEventRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PublishEventRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PublishEventRetired(arg_kind) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PublishEvent"
       /\ packet.payload.kind = arg_kind
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Retired"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "IngressNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PublishEventRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PublishEventRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PublishEventStopped(arg_kind) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PublishEvent"
       /\ packet.payload.kind = arg_kind
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Stopped"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "IngressNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PublishEventStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PublishEventStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AcceptWithCompletionIdleQueued(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AcceptWithCompletion"
       /\ packet.payload.input_id = arg_input_id
       /\ packet.payload.request_immediate_processing = arg_request_immediate_processing
       /\ packet.payload.interrupt_yielding = arg_interrupt_yielding
       /\ packet.payload.wake_if_idle = arg_wake_if_idle
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ (packet.payload.request_immediate_processing = FALSE)
       /\ (packet.payload.interrupt_yielding = FALSE)
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "IngressAccepted", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithCompletionIdleQueued"], [machine |-> "meerkat", variant |-> "PostAdmissionSignal", payload |-> [signal |-> "WakeLoop"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithCompletionIdleQueued"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AcceptWithCompletionIdleQueued", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AcceptWithCompletionIdleImmediate(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AcceptWithCompletion"
       /\ packet.payload.input_id = arg_input_id
       /\ packet.payload.request_immediate_processing = arg_request_immediate_processing
       /\ packet.payload.interrupt_yielding = arg_interrupt_yielding
       /\ packet.payload.wake_if_idle = arg_wake_if_idle
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ (packet.payload.request_immediate_processing = TRUE)
       /\ (packet.payload.interrupt_yielding = FALSE)
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "IngressAccepted", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithCompletionIdleImmediate"], [machine |-> "meerkat", variant |-> "PostAdmissionSignal", payload |-> [signal |-> "RequestImmediateProcessing"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithCompletionIdleImmediate"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AcceptWithCompletionIdleImmediate", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AcceptWithCompletionAttachedImmediate(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AcceptWithCompletion"
       /\ packet.payload.input_id = arg_input_id
       /\ packet.payload.request_immediate_processing = arg_request_immediate_processing
       /\ packet.payload.interrupt_yielding = arg_interrupt_yielding
       /\ packet.payload.wake_if_idle = arg_wake_if_idle
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ (packet.payload.request_immediate_processing = TRUE)
       /\ (packet.payload.interrupt_yielding = FALSE)
       /\ meerkat_phase' = "Running"
       /\ meerkat_current_run_id' = Some(packet.payload.run_id)
       /\ meerkat_pre_run_phase' = Some("attached")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "IngressAccepted", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithCompletionAttachedImmediate"], [machine |-> "meerkat", variant |-> "PostAdmissionSignal", payload |-> [signal |-> "RequestImmediateProcessing"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithCompletionAttachedImmediate"], [machine |-> "meerkat", variant |-> "SubmitRunPrimitive", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithCompletionAttachedImmediate"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AcceptWithCompletionAttachedImmediate", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AcceptWithCompletionAttachedQueued(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AcceptWithCompletion"
       /\ packet.payload.input_id = arg_input_id
       /\ packet.payload.request_immediate_processing = arg_request_immediate_processing
       /\ packet.payload.interrupt_yielding = arg_interrupt_yielding
       /\ packet.payload.wake_if_idle = arg_wake_if_idle
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ (packet.payload.request_immediate_processing = FALSE)
       /\ (packet.payload.interrupt_yielding = FALSE)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "IngressAccepted", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithCompletionAttachedQueued"], [machine |-> "meerkat", variant |-> "PostAdmissionSignal", payload |-> [signal |-> "WakeLoop"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithCompletionAttachedQueued"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AcceptWithCompletionAttachedQueued", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AcceptWithCompletionRunningQueuedPassive(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AcceptWithCompletion"
       /\ packet.payload.input_id = arg_input_id
       /\ packet.payload.request_immediate_processing = arg_request_immediate_processing
       /\ packet.payload.interrupt_yielding = arg_interrupt_yielding
       /\ packet.payload.wake_if_idle = arg_wake_if_idle
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ (packet.payload.request_immediate_processing = FALSE)
       /\ (packet.payload.interrupt_yielding = FALSE)
       /\ (packet.payload.wake_if_idle = FALSE)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "IngressAccepted", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithCompletionRunningQueuedPassive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AcceptWithCompletionRunningQueuedPassive", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AcceptWithCompletionRunningQueuedWakeIfIdle(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AcceptWithCompletion"
       /\ packet.payload.input_id = arg_input_id
       /\ packet.payload.request_immediate_processing = arg_request_immediate_processing
       /\ packet.payload.interrupt_yielding = arg_interrupt_yielding
       /\ packet.payload.wake_if_idle = arg_wake_if_idle
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ (packet.payload.request_immediate_processing = FALSE)
       /\ (packet.payload.interrupt_yielding = FALSE)
       /\ (packet.payload.wake_if_idle = TRUE)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "IngressAccepted", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithCompletionRunningQueuedWakeIfIdle"], [machine |-> "meerkat", variant |-> "PostAdmissionSignal", payload |-> [signal |-> "WakeLoop"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithCompletionRunningQueuedWakeIfIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AcceptWithCompletionRunningQueuedWakeIfIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AcceptWithCompletionRunningInterruptYielding(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AcceptWithCompletion"
       /\ packet.payload.input_id = arg_input_id
       /\ packet.payload.request_immediate_processing = arg_request_immediate_processing
       /\ packet.payload.interrupt_yielding = arg_interrupt_yielding
       /\ packet.payload.wake_if_idle = arg_wake_if_idle
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ (packet.payload.request_immediate_processing = FALSE)
       /\ (packet.payload.interrupt_yielding = TRUE)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "IngressAccepted", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithCompletionRunningInterruptYielding"], [machine |-> "meerkat", variant |-> "PostAdmissionSignal", payload |-> [signal |-> "InterruptYielding"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithCompletionRunningInterruptYielding"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AcceptWithCompletionRunningInterruptYielding", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AcceptWithCompletionRunningImmediate(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AcceptWithCompletion"
       /\ packet.payload.input_id = arg_input_id
       /\ packet.payload.request_immediate_processing = arg_request_immediate_processing
       /\ packet.payload.interrupt_yielding = arg_interrupt_yielding
       /\ packet.payload.wake_if_idle = arg_wake_if_idle
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ (packet.payload.request_immediate_processing = TRUE)
       /\ (packet.payload.interrupt_yielding = FALSE)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "IngressAccepted", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithCompletionRunningImmediate"], [machine |-> "meerkat", variant |-> "PostAdmissionSignal", payload |-> [signal |-> "RequestImmediateProcessing"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithCompletionRunningImmediate"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AcceptWithCompletionRunningImmediate", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AcceptWithoutWakeIdle(arg_input_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AcceptWithoutWake"
       /\ packet.payload.input_id = arg_input_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "IngressAccepted", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithoutWakeIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AcceptWithoutWakeIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AcceptWithoutWakeAttached(arg_input_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AcceptWithoutWake"
       /\ packet.payload.input_id = arg_input_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "IngressAccepted", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithoutWakeAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AcceptWithoutWakeAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AcceptWithoutWakeRunning(arg_input_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AcceptWithoutWake"
       /\ packet.payload.input_id = arg_input_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "IngressAccepted", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithoutWakeRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AcceptWithoutWakeRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ClassifyExternalEnvelopeAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ClassifyExternalEnvelope"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EnqueueClassifiedEntry", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyExternalEnvelopeAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ClassifyExternalEnvelopeAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ClassifyExternalEnvelopeRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ClassifyExternalEnvelope"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EnqueueClassifiedEntry", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyExternalEnvelopeRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ClassifyExternalEnvelopeRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ClassifyPlainEventAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ClassifyPlainEvent"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EnqueueClassifiedEntry", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyPlainEventAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ClassifyPlainEventAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ClassifyPlainEventRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ClassifyPlainEvent"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EnqueueClassifiedEntry", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyPlainEventRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ClassifyPlainEventRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PrepareIdle(arg_session_id, arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Prepare"
       /\ packet.payload.session_id = arg_session_id
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ meerkat_current_run_id' = Some(packet.payload.run_id)
       /\ meerkat_pre_run_phase' = Some("idle")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "SubmitRunPrimitive", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PrepareIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PrepareIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PrepareAttached(arg_session_id, arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Prepare"
       /\ packet.payload.session_id = arg_session_id
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ meerkat_current_run_id' = Some(packet.payload.run_id)
       /\ meerkat_pre_run_phase' = Some("attached")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "SubmitRunPrimitive", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PrepareAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PrepareAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_DrainQueuedRunRetired(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "DrainQueuedRun"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ meerkat_phase' = "Running"
       /\ meerkat_current_run_id' = Some(packet.payload.run_id)
       /\ meerkat_pre_run_phase' = Some("retired")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "SubmitRunPrimitive", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "DrainQueuedRunRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "DrainQueuedRunRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StartConversationRunAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StartConversationRun"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "SubmitRunPrimitive", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StartConversationRunAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StartConversationRunAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StartImmediateAppendAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StartImmediateAppend"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "SubmitRunPrimitive", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StartImmediateAppendAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StartImmediateAppendAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StartImmediateContextAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StartImmediateContext"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "SubmitRunPrimitive", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StartImmediateContextAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StartImmediateContextAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CommitRunningToIdle(arg_input_id, arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Commit"
       /\ packet.payload.input_id = arg_input_id
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_pre_run_phase = Some("idle"))
       /\ (meerkat_current_run_id = Some(packet.payload.run_id))
       /\ meerkat_phase' = "Idle"
       /\ meerkat_current_run_id' = None
       /\ meerkat_pre_run_phase' = None
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CommitRunningToIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CommitRunningToAttached(arg_input_id, arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Commit"
       /\ packet.payload.input_id = arg_input_id
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_pre_run_phase = Some("attached"))
       /\ (meerkat_current_run_id = Some(packet.payload.run_id))
       /\ meerkat_phase' = "Attached"
       /\ meerkat_current_run_id' = None
       /\ meerkat_pre_run_phase' = None
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CommitRunningToAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CommitRunningToRetired(arg_input_id, arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Commit"
       /\ packet.payload.input_id = arg_input_id
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_pre_run_phase = Some("retired"))
       /\ (meerkat_current_run_id = Some(packet.payload.run_id))
       /\ meerkat_phase' = "Retired"
       /\ meerkat_current_run_id' = None
       /\ meerkat_pre_run_phase' = None
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CommitRunningToRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_FailRunningToIdle(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Fail"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_pre_run_phase = Some("idle"))
       /\ (meerkat_current_run_id = Some(packet.payload.run_id))
       /\ meerkat_phase' = "Idle"
       /\ meerkat_current_run_id' = None
       /\ meerkat_pre_run_phase' = None
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RecordTerminalOutcome", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FailRunningToIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "FailRunningToIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_FailRunningToAttached(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Fail"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_pre_run_phase = Some("attached"))
       /\ (meerkat_current_run_id = Some(packet.payload.run_id))
       /\ meerkat_phase' = "Attached"
       /\ meerkat_current_run_id' = None
       /\ meerkat_pre_run_phase' = None
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RecordTerminalOutcome", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FailRunningToAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "FailRunningToAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_FailRunningToRetired(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Fail"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_pre_run_phase = Some("retired"))
       /\ (meerkat_current_run_id = Some(packet.payload.run_id))
       /\ meerkat_phase' = "Retired"
       /\ meerkat_current_run_id' = None
       /\ meerkat_pre_run_phase' = None
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RecordTerminalOutcome", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FailRunningToRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "FailRunningToRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StageAddAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StageAdd"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EmitExternalToolDelta", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StageAddAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StageAddAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StageAddRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StageAdd"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EmitExternalToolDelta", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StageAddRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StageAddRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StageRemoveAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StageRemove"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EmitExternalToolDelta", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StageRemoveAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StageRemoveAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StageRemoveRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StageRemove"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EmitExternalToolDelta", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StageRemoveRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StageRemoveRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StageReloadAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StageReload"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EmitExternalToolDelta", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StageReloadAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StageReloadAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StageReloadRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StageReload"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EmitExternalToolDelta", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StageReloadRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StageReloadRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ApplySurfaceBoundaryAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ApplySurfaceBoundary"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "ScheduleSurfaceCompletion", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ApplySurfaceBoundaryAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ApplySurfaceBoundaryAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ApplySurfaceBoundaryRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ApplySurfaceBoundary"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "ScheduleSurfaceCompletion", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ApplySurfaceBoundaryRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ApplySurfaceBoundaryRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PendingSucceededAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PendingSucceeded"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EmitExternalToolDelta", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PendingSucceededAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PendingSucceededAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PendingSucceededRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PendingSucceeded"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EmitExternalToolDelta", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PendingSucceededRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PendingSucceededRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PendingFailedAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PendingFailed"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EmitExternalToolDelta", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PendingFailedAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PendingFailedAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PendingFailedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PendingFailed"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EmitExternalToolDelta", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PendingFailedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PendingFailedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CallStartedAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "CallStarted"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CallStartedAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CallStartedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "CallStarted"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CallStartedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CallFinishedAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "CallFinished"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CallFinishedAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CallFinishedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "CallFinished"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CallFinishedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_FinalizeRemovalCleanAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "FinalizeRemovalClean"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EmitExternalToolDelta", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FinalizeRemovalCleanAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "FinalizeRemovalCleanAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_FinalizeRemovalCleanRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "FinalizeRemovalClean"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EmitExternalToolDelta", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FinalizeRemovalCleanRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "FinalizeRemovalCleanRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_FinalizeRemovalForcedAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "FinalizeRemovalForced"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EmitExternalToolDelta", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FinalizeRemovalForcedAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "FinalizeRemovalForcedAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_FinalizeRemovalForcedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "FinalizeRemovalForced"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EmitExternalToolDelta", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FinalizeRemovalForcedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "FinalizeRemovalForcedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_SnapshotAlignedAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "SnapshotAligned"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EmitExternalToolDelta", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "SnapshotAlignedAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "SnapshotAlignedAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_SnapshotAlignedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "SnapshotAligned"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EmitExternalToolDelta", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "SnapshotAlignedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "SnapshotAlignedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ShutdownSurfaceAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ShutdownSurface"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EmitExternalToolDelta", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ShutdownSurfaceAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ShutdownSurfaceAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ShutdownSurfaceRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ShutdownSurface"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EmitExternalToolDelta", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ShutdownSurfaceRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ShutdownSurfaceRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RecycleFromIdleOrRetired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Recycle"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle" \/ meerkat_phase = "Retired"
       /\ (meerkat_active_runtime_id # None)
       /\ meerkat_phase' = "Idle"
       /\ meerkat_active_fence_token' = None
       /\ meerkat_current_run_id' = None
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "InitiateRecycle", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RecycleFromIdleOrRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RecycleFromIdleOrRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RecycleFromAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Recycle"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_active_runtime_id # None)
       /\ meerkat_phase' = "Attached"
       /\ meerkat_active_fence_token' = None
       /\ meerkat_current_run_id' = None
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "InitiateRecycle", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RecycleFromAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RecycleFromAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ProjectRealtimeIntentIdle(arg_present) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ProjectRealtimeIntent"
       /\ packet.payload.present = arg_present
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ meerkat_realtime_intent_present' = packet.payload.present
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RealtimeIntentProjected", payload |-> [present |-> packet.payload.present], effect_id |-> (model_step_count + 1), source_transition |-> "ProjectRealtimeIntentIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ProjectRealtimeIntentIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ProjectRealtimeIntentAttached(arg_present) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ProjectRealtimeIntent"
       /\ packet.payload.present = arg_present
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ meerkat_realtime_intent_present' = packet.payload.present
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RealtimeIntentProjected", payload |-> [present |-> packet.payload.present], effect_id |-> (model_step_count + 1), source_transition |-> "ProjectRealtimeIntentAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ProjectRealtimeIntentAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ProjectRealtimeIntentRunning(arg_present) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ProjectRealtimeIntent"
       /\ packet.payload.present = arg_present
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ meerkat_realtime_intent_present' = packet.payload.present
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RealtimeIntentProjected", payload |-> [present |-> packet.payload.present], effect_id |-> (model_step_count + 1), source_transition |-> "ProjectRealtimeIntentRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ProjectRealtimeIntentRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ProjectRealtimeIntentRetired(arg_present) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ProjectRealtimeIntent"
       /\ packet.payload.present = arg_present
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Retired"
       /\ meerkat_realtime_intent_present' = packet.payload.present
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RealtimeIntentProjected", payload |-> [present |-> packet.payload.present], effect_id |-> (model_step_count + 1), source_transition |-> "ProjectRealtimeIntentRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ProjectRealtimeIntentRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ProjectRealtimeIntentStopped(arg_present) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ProjectRealtimeIntent"
       /\ packet.payload.present = arg_present
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_realtime_intent_present' = packet.payload.present
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RealtimeIntentProjected", payload |-> [present |-> packet.payload.present], effect_id |-> (model_step_count + 1), source_transition |-> "ProjectRealtimeIntentStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ProjectRealtimeIntentStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_BeginRealtimeBindingIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "BeginRealtimeBinding"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Idle")
       /\ meerkat_phase' = "Idle"
       /\ meerkat_realtime_binding_state' = "BindingNotReady"
       /\ meerkat_realtime_binding_authority_epoch' = Some(meerkat_realtime_next_authority_epoch)
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RealtimeBindingRotated", payload |-> [authority_epoch |-> (IF "value" \in DOMAIN Some(meerkat_realtime_next_authority_epoch) THEN Some(meerkat_realtime_next_authority_epoch)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRealtimeBindingIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "BeginRealtimeBindingIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_BeginRealtimeBindingAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "BeginRealtimeBinding"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Idle")
       /\ meerkat_phase' = "Attached"
       /\ meerkat_realtime_binding_state' = "BindingNotReady"
       /\ meerkat_realtime_binding_authority_epoch' = Some(meerkat_realtime_next_authority_epoch)
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RealtimeBindingRotated", payload |-> [authority_epoch |-> (IF "value" \in DOMAIN Some(meerkat_realtime_next_authority_epoch) THEN Some(meerkat_realtime_next_authority_epoch)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRealtimeBindingAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "BeginRealtimeBindingAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_BeginRealtimeBindingRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "BeginRealtimeBinding"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Idle")
       /\ meerkat_phase' = "Running"
       /\ meerkat_realtime_binding_state' = "BindingNotReady"
       /\ meerkat_realtime_binding_authority_epoch' = Some(meerkat_realtime_next_authority_epoch)
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RealtimeBindingRotated", payload |-> [authority_epoch |-> (IF "value" \in DOMAIN Some(meerkat_realtime_next_authority_epoch) THEN Some(meerkat_realtime_next_authority_epoch)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRealtimeBindingRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "BeginRealtimeBindingRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_BeginRealtimeBindingRetired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "BeginRealtimeBinding"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Idle")
       /\ meerkat_phase' = "Retired"
       /\ meerkat_realtime_binding_state' = "BindingNotReady"
       /\ meerkat_realtime_binding_authority_epoch' = Some(meerkat_realtime_next_authority_epoch)
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RealtimeBindingRotated", payload |-> [authority_epoch |-> (IF "value" \in DOMAIN Some(meerkat_realtime_next_authority_epoch) THEN Some(meerkat_realtime_next_authority_epoch)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRealtimeBindingRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "BeginRealtimeBindingRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_BeginRealtimeBindingStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "BeginRealtimeBinding"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Idle")
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_realtime_binding_state' = "BindingNotReady"
       /\ meerkat_realtime_binding_authority_epoch' = Some(meerkat_realtime_next_authority_epoch)
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RealtimeBindingRotated", payload |-> [authority_epoch |-> (IF "value" \in DOMAIN Some(meerkat_realtime_next_authority_epoch) THEN Some(meerkat_realtime_next_authority_epoch)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "BeginRealtimeBindingStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "BeginRealtimeBindingStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ReplaceRealtimeBindingIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ReplaceRealtimeBinding"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Idle")
       /\ meerkat_phase' = "Idle"
       /\ meerkat_realtime_binding_state' = "ReplacementPending"
       /\ meerkat_realtime_binding_authority_epoch' = Some(meerkat_realtime_next_authority_epoch)
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RealtimeBindingRotated", payload |-> [authority_epoch |-> (IF "value" \in DOMAIN Some(meerkat_realtime_next_authority_epoch) THEN Some(meerkat_realtime_next_authority_epoch)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "ReplaceRealtimeBindingIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ReplaceRealtimeBindingIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ReplaceRealtimeBindingAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ReplaceRealtimeBinding"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Idle")
       /\ meerkat_phase' = "Attached"
       /\ meerkat_realtime_binding_state' = "ReplacementPending"
       /\ meerkat_realtime_binding_authority_epoch' = Some(meerkat_realtime_next_authority_epoch)
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RealtimeBindingRotated", payload |-> [authority_epoch |-> (IF "value" \in DOMAIN Some(meerkat_realtime_next_authority_epoch) THEN Some(meerkat_realtime_next_authority_epoch)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "ReplaceRealtimeBindingAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ReplaceRealtimeBindingAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ReplaceRealtimeBindingRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ReplaceRealtimeBinding"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Idle")
       /\ meerkat_phase' = "Running"
       /\ meerkat_realtime_binding_state' = "ReplacementPending"
       /\ meerkat_realtime_binding_authority_epoch' = Some(meerkat_realtime_next_authority_epoch)
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RealtimeBindingRotated", payload |-> [authority_epoch |-> (IF "value" \in DOMAIN Some(meerkat_realtime_next_authority_epoch) THEN Some(meerkat_realtime_next_authority_epoch)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "ReplaceRealtimeBindingRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ReplaceRealtimeBindingRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ReplaceRealtimeBindingRetired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ReplaceRealtimeBinding"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Idle")
       /\ meerkat_phase' = "Retired"
       /\ meerkat_realtime_binding_state' = "ReplacementPending"
       /\ meerkat_realtime_binding_authority_epoch' = Some(meerkat_realtime_next_authority_epoch)
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RealtimeBindingRotated", payload |-> [authority_epoch |-> (IF "value" \in DOMAIN Some(meerkat_realtime_next_authority_epoch) THEN Some(meerkat_realtime_next_authority_epoch)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "ReplaceRealtimeBindingRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ReplaceRealtimeBindingRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ReplaceRealtimeBindingStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ReplaceRealtimeBinding"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Idle")
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_realtime_binding_state' = "ReplacementPending"
       /\ meerkat_realtime_binding_authority_epoch' = Some(meerkat_realtime_next_authority_epoch)
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RealtimeBindingRotated", payload |-> [authority_epoch |-> (IF "value" \in DOMAIN Some(meerkat_realtime_next_authority_epoch) THEN Some(meerkat_realtime_next_authority_epoch)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "ReplaceRealtimeBindingStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ReplaceRealtimeBindingStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_DetachRealtimeBindingIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "DetachRealtimeBinding"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ meerkat_realtime_binding_state' = "Unbound"
       /\ meerkat_realtime_binding_authority_epoch' = None
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "DetachRealtimeBindingIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_DetachRealtimeBindingAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "DetachRealtimeBinding"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ meerkat_realtime_binding_state' = "Unbound"
       /\ meerkat_realtime_binding_authority_epoch' = None
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "DetachRealtimeBindingAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_DetachRealtimeBindingRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "DetachRealtimeBinding"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ meerkat_realtime_binding_state' = "Unbound"
       /\ meerkat_realtime_binding_authority_epoch' = None
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "DetachRealtimeBindingRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_DetachRealtimeBindingRetired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "DetachRealtimeBinding"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Retired"
       /\ meerkat_realtime_binding_state' = "Unbound"
       /\ meerkat_realtime_binding_authority_epoch' = None
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "DetachRealtimeBindingRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_DetachRealtimeBindingStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "DetachRealtimeBinding"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_realtime_binding_state' = "Unbound"
       /\ meerkat_realtime_binding_authority_epoch' = None
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "DetachRealtimeBindingStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RequireRealtimeReattachIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RequireRealtimeReattach"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ meerkat_realtime_binding_state' = "Unbound"
       /\ meerkat_realtime_binding_authority_epoch' = None
       /\ meerkat_realtime_reattach_required' = TRUE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RequireRealtimeReattachIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RequireRealtimeReattachAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RequireRealtimeReattach"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ meerkat_realtime_binding_state' = "Unbound"
       /\ meerkat_realtime_binding_authority_epoch' = None
       /\ meerkat_realtime_reattach_required' = TRUE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RequireRealtimeReattachAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RequireRealtimeReattachRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RequireRealtimeReattach"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ meerkat_realtime_binding_state' = "Unbound"
       /\ meerkat_realtime_binding_authority_epoch' = None
       /\ meerkat_realtime_reattach_required' = TRUE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RequireRealtimeReattachRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RequireRealtimeReattachRetired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RequireRealtimeReattach"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Retired"
       /\ meerkat_realtime_binding_state' = "Unbound"
       /\ meerkat_realtime_binding_authority_epoch' = None
       /\ meerkat_realtime_reattach_required' = TRUE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RequireRealtimeReattachRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RequireRealtimeReattachStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RequireRealtimeReattach"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_realtime_binding_state' = "Unbound"
       /\ meerkat_realtime_binding_authority_epoch' = None
       /\ meerkat_realtime_reattach_required' = TRUE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RequireRealtimeReattachStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PublishRealtimeSignalIdle(arg_authority_epoch, arg_next_binding_state) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PublishRealtimeSignal"
       /\ packet.payload.authority_epoch = arg_authority_epoch
       /\ packet.payload.next_binding_state = arg_next_binding_state
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_realtime_binding_authority_epoch = Some(packet.payload.authority_epoch))
       /\ (meerkat_live_topology_phase = "Idle")
       /\ ((packet.payload.next_binding_state = "BindingNotReady") \/ (packet.payload.next_binding_state = "BindingReady") \/ (packet.payload.next_binding_state = "ReplacementPending"))
       /\ meerkat_phase' = "Idle"
       /\ meerkat_realtime_binding_state' = packet.payload.next_binding_state
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_authority_epoch, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PublishRealtimeSignalIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PublishRealtimeSignalAttached(arg_authority_epoch, arg_next_binding_state) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PublishRealtimeSignal"
       /\ packet.payload.authority_epoch = arg_authority_epoch
       /\ packet.payload.next_binding_state = arg_next_binding_state
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_realtime_binding_authority_epoch = Some(packet.payload.authority_epoch))
       /\ (meerkat_live_topology_phase = "Idle")
       /\ ((packet.payload.next_binding_state = "BindingNotReady") \/ (packet.payload.next_binding_state = "BindingReady") \/ (packet.payload.next_binding_state = "ReplacementPending"))
       /\ meerkat_phase' = "Attached"
       /\ meerkat_realtime_binding_state' = packet.payload.next_binding_state
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_authority_epoch, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PublishRealtimeSignalAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PublishRealtimeSignalRunning(arg_authority_epoch, arg_next_binding_state) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PublishRealtimeSignal"
       /\ packet.payload.authority_epoch = arg_authority_epoch
       /\ packet.payload.next_binding_state = arg_next_binding_state
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_realtime_binding_authority_epoch = Some(packet.payload.authority_epoch))
       /\ (meerkat_live_topology_phase = "Idle")
       /\ ((packet.payload.next_binding_state = "BindingNotReady") \/ (packet.payload.next_binding_state = "BindingReady") \/ (packet.payload.next_binding_state = "ReplacementPending"))
       /\ meerkat_phase' = "Running"
       /\ meerkat_realtime_binding_state' = packet.payload.next_binding_state
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_authority_epoch, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PublishRealtimeSignalRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PublishRealtimeSignalRetired(arg_authority_epoch, arg_next_binding_state) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PublishRealtimeSignal"
       /\ packet.payload.authority_epoch = arg_authority_epoch
       /\ packet.payload.next_binding_state = arg_next_binding_state
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_realtime_binding_authority_epoch = Some(packet.payload.authority_epoch))
       /\ (meerkat_live_topology_phase = "Idle")
       /\ ((packet.payload.next_binding_state = "BindingNotReady") \/ (packet.payload.next_binding_state = "BindingReady") \/ (packet.payload.next_binding_state = "ReplacementPending"))
       /\ meerkat_phase' = "Retired"
       /\ meerkat_realtime_binding_state' = packet.payload.next_binding_state
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_authority_epoch, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PublishRealtimeSignalRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PublishRealtimeSignalStopped(arg_authority_epoch, arg_next_binding_state) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PublishRealtimeSignal"
       /\ packet.payload.authority_epoch = arg_authority_epoch
       /\ packet.payload.next_binding_state = arg_next_binding_state
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_realtime_binding_authority_epoch = Some(packet.payload.authority_epoch))
       /\ (meerkat_live_topology_phase = "Idle")
       /\ ((packet.payload.next_binding_state = "BindingNotReady") \/ (packet.payload.next_binding_state = "BindingReady") \/ (packet.payload.next_binding_state = "ReplacementPending"))
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_realtime_binding_state' = packet.payload.next_binding_state
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_authority_epoch, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PublishRealtimeSignalStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerConnectPendingIdle(arg_server_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerConnectPending"
       /\ packet.payload.server_id = arg_server_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "PendingConnect")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "PendingConnect", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerConnectPendingIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerConnectPendingIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerConnectPendingAttached(arg_server_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerConnectPending"
       /\ packet.payload.server_id = arg_server_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "PendingConnect")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "PendingConnect", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerConnectPendingAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerConnectPendingAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerConnectPendingRunning(arg_server_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerConnectPending"
       /\ packet.payload.server_id = arg_server_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "PendingConnect")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "PendingConnect", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerConnectPendingRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerConnectPendingRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerConnectPendingRetired(arg_server_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerConnectPending"
       /\ packet.payload.server_id = arg_server_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Retired"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "PendingConnect")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "PendingConnect", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerConnectPendingRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerConnectPendingRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerConnectPendingStopped(arg_server_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerConnectPending"
       /\ packet.payload.server_id = arg_server_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "PendingConnect")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "PendingConnect", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerConnectPendingStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerConnectPendingStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerConnectedIdle(arg_server_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerConnected"
       /\ packet.payload.server_id = arg_server_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "Connected")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "Connected", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerConnectedIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerConnectedIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerConnectedAttached(arg_server_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerConnected"
       /\ packet.payload.server_id = arg_server_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "Connected")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "Connected", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerConnectedAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerConnectedAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerConnectedRunning(arg_server_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerConnected"
       /\ packet.payload.server_id = arg_server_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "Connected")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "Connected", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerConnectedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerConnectedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerConnectedRetired(arg_server_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerConnected"
       /\ packet.payload.server_id = arg_server_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Retired"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "Connected")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "Connected", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerConnectedRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerConnectedRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerConnectedStopped(arg_server_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerConnected"
       /\ packet.payload.server_id = arg_server_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "Connected")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "Connected", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerConnectedStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerConnectedStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerFailedIdle(arg_server_id, arg_error) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerFailed"
       /\ packet.payload.server_id = arg_server_id
       /\ packet.payload.error = arg_error
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "Failed")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "Failed", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerFailedIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerFailedIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerFailedAttached(arg_server_id, arg_error) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerFailed"
       /\ packet.payload.server_id = arg_server_id
       /\ packet.payload.error = arg_error
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "Failed")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "Failed", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerFailedAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerFailedAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerFailedRunning(arg_server_id, arg_error) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerFailed"
       /\ packet.payload.server_id = arg_server_id
       /\ packet.payload.error = arg_error
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "Failed")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "Failed", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerFailedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerFailedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerFailedRetired(arg_server_id, arg_error) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerFailed"
       /\ packet.payload.server_id = arg_server_id
       /\ packet.payload.error = arg_error
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Retired"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "Failed")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "Failed", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerFailedRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerFailedRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerFailedStopped(arg_server_id, arg_error) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerFailed"
       /\ packet.payload.server_id = arg_server_id
       /\ packet.payload.error = arg_error
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "Failed")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "Failed", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerFailedStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerFailedStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerDisconnectedIdle(arg_server_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerDisconnected"
       /\ packet.payload.server_id = arg_server_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "Disconnected")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "Disconnected", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerDisconnectedIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerDisconnectedIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerDisconnectedAttached(arg_server_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerDisconnected"
       /\ packet.payload.server_id = arg_server_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "Disconnected")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "Disconnected", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerDisconnectedAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerDisconnectedAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerDisconnectedRunning(arg_server_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerDisconnected"
       /\ packet.payload.server_id = arg_server_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "Disconnected")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "Disconnected", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerDisconnectedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerDisconnectedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerDisconnectedRetired(arg_server_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerDisconnected"
       /\ packet.payload.server_id = arg_server_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Retired"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "Disconnected")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "Disconnected", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerDisconnectedRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerDisconnectedRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerDisconnectedStopped(arg_server_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerDisconnected"
       /\ packet.payload.server_id = arg_server_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "Disconnected")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "Disconnected", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerDisconnectedStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerDisconnectedStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerReloadIdle(arg_server_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerReload"
       /\ packet.payload.server_id = arg_server_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "PendingConnect")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerReloadRequested", payload |-> [server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerReloadIdle"], [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "PendingConnect", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerReloadIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerReloadIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerReloadAttached(arg_server_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerReload"
       /\ packet.payload.server_id = arg_server_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "PendingConnect")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerReloadRequested", payload |-> [server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerReloadAttached"], [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "PendingConnect", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerReloadAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerReloadAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerReloadRunning(arg_server_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerReload"
       /\ packet.payload.server_id = arg_server_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "PendingConnect")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerReloadRequested", payload |-> [server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerReloadRunning"], [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "PendingConnect", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerReloadRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerReloadRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerReloadRetired(arg_server_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerReload"
       /\ packet.payload.server_id = arg_server_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Retired"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "PendingConnect")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerReloadRequested", payload |-> [server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerReloadRetired"], [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "PendingConnect", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerReloadRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerReloadRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_McpServerReloadStopped(arg_server_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "McpServerReload"
       /\ packet.payload.server_id = arg_server_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_mcp_server_states' = MapSet(meerkat_mcp_server_states, packet.payload.server_id, "PendingConnect")
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "McpServerReloadRequested", payload |-> [server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerReloadStopped"], [machine |-> "meerkat", variant |-> "McpServerStateChanged", payload |-> [new_state |-> "PendingConnect", server_id |-> packet.payload.server_id], effect_id |-> (model_step_count + 1), source_transition |-> "McpServerReloadStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "McpServerReloadStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_BeginLiveTopologyReconfigureIdle(arg_authority_epoch) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "BeginLiveTopologyReconfigure"
       /\ packet.payload.authority_epoch = arg_authority_epoch
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ (meerkat_realtime_binding_authority_epoch = Some(packet.payload.authority_epoch))
       /\ (meerkat_live_topology_phase = "Idle")
       /\ meerkat_phase' = "Idle"
       /\ meerkat_live_topology_phase' = "Reconfiguring"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginLiveTopologyReconfigureIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "BeginLiveTopologyReconfigureIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_BeginLiveTopologyReconfigureAttached(arg_authority_epoch) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "BeginLiveTopologyReconfigure"
       /\ packet.payload.authority_epoch = arg_authority_epoch
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ (meerkat_realtime_binding_authority_epoch = Some(packet.payload.authority_epoch))
       /\ (meerkat_live_topology_phase = "Idle")
       /\ meerkat_phase' = "Attached"
       /\ meerkat_live_topology_phase' = "Reconfiguring"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginLiveTopologyReconfigureAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "BeginLiveTopologyReconfigureAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_BeginLiveTopologyReconfigureRunning(arg_authority_epoch) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "BeginLiveTopologyReconfigure"
       /\ packet.payload.authority_epoch = arg_authority_epoch
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ (meerkat_realtime_binding_authority_epoch = Some(packet.payload.authority_epoch))
       /\ (meerkat_live_topology_phase = "Idle")
       /\ meerkat_phase' = "Running"
       /\ meerkat_live_topology_phase' = "Reconfiguring"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginLiveTopologyReconfigureRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "BeginLiveTopologyReconfigureRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_BeginLiveTopologyReconfigureRetired(arg_authority_epoch) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "BeginLiveTopologyReconfigure"
       /\ packet.payload.authority_epoch = arg_authority_epoch
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ (meerkat_realtime_binding_authority_epoch = Some(packet.payload.authority_epoch))
       /\ (meerkat_live_topology_phase = "Idle")
       /\ meerkat_phase' = "Retired"
       /\ meerkat_live_topology_phase' = "Reconfiguring"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginLiveTopologyReconfigureRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "BeginLiveTopologyReconfigureRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_BeginLiveTopologyReconfigureStopped(arg_authority_epoch) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "BeginLiveTopologyReconfigure"
       /\ packet.payload.authority_epoch = arg_authority_epoch
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ (meerkat_realtime_binding_authority_epoch = Some(packet.payload.authority_epoch))
       /\ (meerkat_live_topology_phase = "Idle")
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_live_topology_phase' = "Reconfiguring"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginLiveTopologyReconfigureStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "BeginLiveTopologyReconfigureStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_MarkLiveTopologyDetachedIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "MarkLiveTopologyDetached"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Reconfiguring")
       /\ (meerkat_current_run_id = None)
       /\ meerkat_phase' = "Idle"
       /\ meerkat_realtime_binding_state' = "Unbound"
       /\ meerkat_realtime_binding_authority_epoch' = None
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ meerkat_live_topology_phase' = "Detached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkLiveTopologyDetachedIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "MarkLiveTopologyDetachedIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_MarkLiveTopologyDetachedAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "MarkLiveTopologyDetached"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Reconfiguring")
       /\ (meerkat_current_run_id = None)
       /\ meerkat_phase' = "Attached"
       /\ meerkat_realtime_binding_state' = "Unbound"
       /\ meerkat_realtime_binding_authority_epoch' = None
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ meerkat_live_topology_phase' = "Detached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkLiveTopologyDetachedAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "MarkLiveTopologyDetachedAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_MarkLiveTopologyDetachedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "MarkLiveTopologyDetached"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Reconfiguring")
       /\ (meerkat_current_run_id = None)
       /\ meerkat_phase' = "Running"
       /\ meerkat_realtime_binding_state' = "Unbound"
       /\ meerkat_realtime_binding_authority_epoch' = None
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ meerkat_live_topology_phase' = "Detached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkLiveTopologyDetachedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "MarkLiveTopologyDetachedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_MarkLiveTopologyDetachedRetired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "MarkLiveTopologyDetached"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Reconfiguring")
       /\ (meerkat_current_run_id = None)
       /\ meerkat_phase' = "Retired"
       /\ meerkat_realtime_binding_state' = "Unbound"
       /\ meerkat_realtime_binding_authority_epoch' = None
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ meerkat_live_topology_phase' = "Detached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkLiveTopologyDetachedRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "MarkLiveTopologyDetachedRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_MarkLiveTopologyDetachedStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "MarkLiveTopologyDetached"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Reconfiguring")
       /\ (meerkat_current_run_id = None)
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_realtime_binding_state' = "Unbound"
       /\ meerkat_realtime_binding_authority_epoch' = None
       /\ meerkat_realtime_reattach_required' = FALSE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ meerkat_live_topology_phase' = "Detached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkLiveTopologyDetachedStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "MarkLiveTopologyDetachedStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ApplyLiveTopologyIdentityIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ApplyLiveTopologyIdentity"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Detached")
       /\ meerkat_phase' = "Idle"
       /\ meerkat_live_topology_phase' = "HostIdentityApplied"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyLiveTopologyIdentityIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ApplyLiveTopologyIdentityIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ApplyLiveTopologyIdentityAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ApplyLiveTopologyIdentity"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Detached")
       /\ meerkat_phase' = "Attached"
       /\ meerkat_live_topology_phase' = "HostIdentityApplied"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyLiveTopologyIdentityAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ApplyLiveTopologyIdentityAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ApplyLiveTopologyIdentityRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ApplyLiveTopologyIdentity"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Detached")
       /\ meerkat_phase' = "Running"
       /\ meerkat_live_topology_phase' = "HostIdentityApplied"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyLiveTopologyIdentityRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ApplyLiveTopologyIdentityRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ApplyLiveTopologyIdentityRetired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ApplyLiveTopologyIdentity"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Detached")
       /\ meerkat_phase' = "Retired"
       /\ meerkat_live_topology_phase' = "HostIdentityApplied"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyLiveTopologyIdentityRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ApplyLiveTopologyIdentityRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ApplyLiveTopologyIdentityStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ApplyLiveTopologyIdentity"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Detached")
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_live_topology_phase' = "HostIdentityApplied"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyLiveTopologyIdentityStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ApplyLiveTopologyIdentityStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ApplyLiveTopologyVisibilityIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ApplyLiveTopologyVisibility"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "HostIdentityApplied")
       /\ meerkat_phase' = "Idle"
       /\ meerkat_live_topology_phase' = "HostVisibilityApplied"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyLiveTopologyVisibilityIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ApplyLiveTopologyVisibilityIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ApplyLiveTopologyVisibilityAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ApplyLiveTopologyVisibility"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "HostIdentityApplied")
       /\ meerkat_phase' = "Attached"
       /\ meerkat_live_topology_phase' = "HostVisibilityApplied"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyLiveTopologyVisibilityAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ApplyLiveTopologyVisibilityAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ApplyLiveTopologyVisibilityRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ApplyLiveTopologyVisibility"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "HostIdentityApplied")
       /\ meerkat_phase' = "Running"
       /\ meerkat_live_topology_phase' = "HostVisibilityApplied"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyLiveTopologyVisibilityRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ApplyLiveTopologyVisibilityRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ApplyLiveTopologyVisibilityRetired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ApplyLiveTopologyVisibility"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "HostIdentityApplied")
       /\ meerkat_phase' = "Retired"
       /\ meerkat_live_topology_phase' = "HostVisibilityApplied"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyLiveTopologyVisibilityRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ApplyLiveTopologyVisibilityRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ApplyLiveTopologyVisibilityStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ApplyLiveTopologyVisibility"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "HostIdentityApplied")
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_live_topology_phase' = "HostVisibilityApplied"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ApplyLiveTopologyVisibilityStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ApplyLiveTopologyVisibilityStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CompleteLiveTopologyIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "CompleteLiveTopology"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "HostVisibilityApplied")
       /\ meerkat_phase' = "Idle"
       /\ meerkat_live_topology_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteLiveTopologyIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CompleteLiveTopologyIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CompleteLiveTopologyAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "CompleteLiveTopology"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "HostVisibilityApplied")
       /\ meerkat_phase' = "Attached"
       /\ meerkat_live_topology_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteLiveTopologyAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CompleteLiveTopologyAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CompleteLiveTopologyRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "CompleteLiveTopology"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "HostVisibilityApplied")
       /\ meerkat_phase' = "Running"
       /\ meerkat_live_topology_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteLiveTopologyRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CompleteLiveTopologyRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CompleteLiveTopologyRetired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "CompleteLiveTopology"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "HostVisibilityApplied")
       /\ meerkat_phase' = "Retired"
       /\ meerkat_live_topology_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteLiveTopologyRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CompleteLiveTopologyRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CompleteLiveTopologyStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "CompleteLiveTopology"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "HostVisibilityApplied")
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_live_topology_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteLiveTopologyStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CompleteLiveTopologyStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AbortLiveTopologyBeforeDetachIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AbortLiveTopologyBeforeDetach"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Reconfiguring")
       /\ meerkat_phase' = "Idle"
       /\ meerkat_live_topology_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AbortLiveTopologyBeforeDetachIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AbortLiveTopologyBeforeDetachIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AbortLiveTopologyBeforeDetachAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AbortLiveTopologyBeforeDetach"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Reconfiguring")
       /\ meerkat_phase' = "Attached"
       /\ meerkat_live_topology_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AbortLiveTopologyBeforeDetachAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AbortLiveTopologyBeforeDetachAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AbortLiveTopologyBeforeDetachRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AbortLiveTopologyBeforeDetach"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Reconfiguring")
       /\ meerkat_phase' = "Running"
       /\ meerkat_live_topology_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AbortLiveTopologyBeforeDetachRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AbortLiveTopologyBeforeDetachRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AbortLiveTopologyBeforeDetachRetired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AbortLiveTopologyBeforeDetach"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Reconfiguring")
       /\ meerkat_phase' = "Retired"
       /\ meerkat_live_topology_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AbortLiveTopologyBeforeDetachRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AbortLiveTopologyBeforeDetachRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AbortLiveTopologyBeforeDetachStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AbortLiveTopologyBeforeDetach"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ (meerkat_live_topology_phase = "Reconfiguring")
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_live_topology_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AbortLiveTopologyBeforeDetachStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AbortLiveTopologyBeforeDetachStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_FailLiveTopologyAfterDetachIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "FailLiveTopologyAfterDetach"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ ((meerkat_live_topology_phase = "Detached") \/ (meerkat_live_topology_phase = "HostIdentityApplied") \/ (meerkat_live_topology_phase = "HostVisibilityApplied"))
       /\ meerkat_phase' = "Idle"
       /\ meerkat_realtime_binding_state' = "Unbound"
       /\ meerkat_realtime_binding_authority_epoch' = None
       /\ meerkat_realtime_reattach_required' = TRUE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ meerkat_live_topology_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FailLiveTopologyAfterDetachIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "FailLiveTopologyAfterDetachIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_FailLiveTopologyAfterDetachAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "FailLiveTopologyAfterDetach"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ ((meerkat_live_topology_phase = "Detached") \/ (meerkat_live_topology_phase = "HostIdentityApplied") \/ (meerkat_live_topology_phase = "HostVisibilityApplied"))
       /\ meerkat_phase' = "Attached"
       /\ meerkat_realtime_binding_state' = "Unbound"
       /\ meerkat_realtime_binding_authority_epoch' = None
       /\ meerkat_realtime_reattach_required' = TRUE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ meerkat_live_topology_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FailLiveTopologyAfterDetachAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "FailLiveTopologyAfterDetachAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_FailLiveTopologyAfterDetachRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "FailLiveTopologyAfterDetach"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ ((meerkat_live_topology_phase = "Detached") \/ (meerkat_live_topology_phase = "HostIdentityApplied") \/ (meerkat_live_topology_phase = "HostVisibilityApplied"))
       /\ meerkat_phase' = "Running"
       /\ meerkat_realtime_binding_state' = "Unbound"
       /\ meerkat_realtime_binding_authority_epoch' = None
       /\ meerkat_realtime_reattach_required' = TRUE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ meerkat_live_topology_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FailLiveTopologyAfterDetachRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "FailLiveTopologyAfterDetachRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_FailLiveTopologyAfterDetachRetired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "FailLiveTopologyAfterDetach"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ ((meerkat_live_topology_phase = "Detached") \/ (meerkat_live_topology_phase = "HostIdentityApplied") \/ (meerkat_live_topology_phase = "HostVisibilityApplied"))
       /\ meerkat_phase' = "Retired"
       /\ meerkat_realtime_binding_state' = "Unbound"
       /\ meerkat_realtime_binding_authority_epoch' = None
       /\ meerkat_realtime_reattach_required' = TRUE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ meerkat_live_topology_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FailLiveTopologyAfterDetachRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "FailLiveTopologyAfterDetachRetired", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_FailLiveTopologyAfterDetachStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "FailLiveTopologyAfterDetach"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Stopped"
       /\ (meerkat_session_id # None)
       /\ ((meerkat_live_topology_phase = "Detached") \/ (meerkat_live_topology_phase = "HostIdentityApplied") \/ (meerkat_live_topology_phase = "HostVisibilityApplied"))
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_realtime_binding_state' = "Unbound"
       /\ meerkat_realtime_binding_authority_epoch' = None
       /\ meerkat_realtime_reattach_required' = TRUE
       /\ meerkat_realtime_next_authority_epoch' = (meerkat_realtime_next_authority_epoch + 1)
       /\ meerkat_live_topology_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "LiveTopologyPhaseChanged", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FailLiveTopologyAfterDetachStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "FailLiveTopologyAfterDetachStopped", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_fence_requires_bound_runtime == ((meerkat_active_fence_token = None) \/ (meerkat_active_runtime_id # None))
meerkat_running_has_current_run == ((meerkat_phase # "Running") \/ (meerkat_current_run_id # None))
meerkat_current_run_only_while_running_or_retired == ((meerkat_current_run_id = None) \/ (meerkat_phase = "Running") \/ (meerkat_phase = "Retired"))
meerkat_realtime_binding_epoch_consistency == ((meerkat_realtime_binding_state = "Unbound") = (meerkat_realtime_binding_authority_epoch = None))

mob_SpawnRunning(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation, arg_external_addressable) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Spawn"
       /\ packet.payload.agent_identity = arg_agent_identity
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.generation = arg_generation
       /\ packet.payload.external_addressable = arg_external_addressable
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_coordinator_bound = TRUE)
       /\ mob_phase' = "Running"
       /\ mob_live_runtime_ids' = (mob_live_runtime_ids \cup {packet.payload.agent_runtime_id})
       /\ mob_externally_addressable_runtime_ids' = IF packet.payload.external_addressable THEN (mob_externally_addressable_runtime_ids \cup {packet.payload.agent_runtime_id}) ELSE (mob_externally_addressable_runtime_ids \ {packet.payload.agent_runtime_id})
       /\ mob_runtime_fence_tokens' = MapSet(mob_runtime_fence_tokens, packet.payload.agent_runtime_id, packet.payload.fence_token)
       /\ mob_identity_to_runtime' = MapSet(mob_identity_to_runtime, packet.payload.agent_identity, packet.payload.agent_runtime_id)
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "PrepareBindings", payload |-> [agent_runtime_id |-> packet.payload.agent_runtime_id, fence_token |-> packet.payload.fence_token, generation |-> packet.payload.generation], source_kind |-> "route", source_route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeBinding", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "PrepareBindings", payload |-> [agent_runtime_id |-> packet.payload.agent_runtime_id, fence_token |-> packet.payload.fence_token, generation |-> packet.payload.generation], source_kind |-> "route", source_route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeBinding", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", effect |-> "RequestRuntimeBinding", target_machine |-> "meerkat", target_input |-> "PrepareBindings", payload |-> [agent_runtime_id |-> packet.payload.agent_runtime_id, fence_token |-> packet.payload.fence_token, generation |-> packet.payload.generation], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "SpawnRunning"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestRuntimeBinding", payload |-> [agent_identity |-> packet.payload.agent_identity, agent_runtime_id |-> packet.payload.agent_runtime_id, fence_token |-> packet.payload.fence_token, generation |-> packet.payload.generation], effect_id |-> (model_step_count + 1), source_transition |-> "SpawnRunning"], [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [kind |-> "spawned"], effect_id |-> (model_step_count + 1), source_transition |-> "SpawnRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SpawnRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_ObserveRuntimeReady(arg_agent_runtime_id, arg_fence_token) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ObserveRuntimeReady"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ObserveRuntimeReady", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_SubmitWorkRunningExternal(arg_agent_runtime_id, arg_fence_token, arg_work_id, arg_origin) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubmitWork"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.origin = arg_origin
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_live_runtime_ids # {})
       /\ (packet.payload.agent_runtime_id \in mob_live_runtime_ids)
       /\ (packet.payload.origin = "External")
       /\ (packet.payload.agent_runtime_id \in mob_externally_addressable_runtime_ids)
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "Ingest", payload |-> [origin |-> packet.payload.origin, runtime_id |-> packet.payload.agent_runtime_id, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "work_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeIngress", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "Ingest", payload |-> [origin |-> packet.payload.origin, runtime_id |-> packet.payload.agent_runtime_id, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "work_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeIngress", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "work_request_reaches_meerkat", source_machine |-> "mob", effect |-> "RequestRuntimeIngress", target_machine |-> "meerkat", target_input |-> "Ingest", payload |-> [origin |-> packet.payload.origin, runtime_id |-> packet.payload.agent_runtime_id, work_id |-> packet.payload.work_id], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "SubmitWorkRunningExternal"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestRuntimeIngress", payload |-> [agent_runtime_id |-> packet.payload.agent_runtime_id, fence_token |-> packet.payload.fence_token, origin |-> packet.payload.origin, work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "SubmitWorkRunningExternal"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SubmitWorkRunningExternal", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_SubmitWorkRunningInternal(arg_agent_runtime_id, arg_fence_token, arg_work_id, arg_origin) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubmitWork"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.work_id = arg_work_id
       /\ packet.payload.origin = arg_origin
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_live_runtime_ids # {})
       /\ (packet.payload.agent_runtime_id \in mob_live_runtime_ids)
       /\ (packet.payload.origin = "Internal")
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "Ingest", payload |-> [origin |-> packet.payload.origin, runtime_id |-> packet.payload.agent_runtime_id, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "work_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeIngress", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "Ingest", payload |-> [origin |-> packet.payload.origin, runtime_id |-> packet.payload.agent_runtime_id, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "work_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeIngress", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "work_request_reaches_meerkat", source_machine |-> "mob", effect |-> "RequestRuntimeIngress", target_machine |-> "meerkat", target_input |-> "Ingest", payload |-> [origin |-> packet.payload.origin, runtime_id |-> packet.payload.agent_runtime_id, work_id |-> packet.payload.work_id], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "SubmitWorkRunningInternal"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestRuntimeIngress", payload |-> [agent_runtime_id |-> packet.payload.agent_runtime_id, fence_token |-> packet.payload.fence_token, origin |-> packet.payload.origin, work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "SubmitWorkRunningInternal"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SubmitWorkRunningInternal", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RetireMember(arg_agent_runtime_id, arg_fence_token) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RetireMember"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (packet.payload.agent_runtime_id \in mob_live_runtime_ids)
       /\ mob_phase' = "Running"
       /\ mob_member_state_markers' = MapSet(mob_member_state_markers, packet.payload.agent_runtime_id, "Retiring")
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "Retire", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeRetire", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "Retire", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeRetire", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", effect |-> "RequestRuntimeRetire", target_machine |-> "meerkat", target_input |-> "Retire", payload |-> [tag |-> "unit"], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "RetireMember"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestRuntimeRetire", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RetireMember"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RetireMember", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_ObserveRuntimeRetired(arg_agent_runtime_id, arg_fence_token) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ObserveRuntimeRetired"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (packet.payload.agent_runtime_id \in mob_live_runtime_ids)
       /\ mob_phase' = "Stopped"
       /\ mob_live_runtime_ids' = (mob_live_runtime_ids \ {packet.payload.agent_runtime_id})
       /\ mob_externally_addressable_runtime_ids' = (mob_externally_addressable_runtime_ids \ {packet.payload.agent_runtime_id})
       /\ mob_runtime_fence_tokens' = MapRemove(mob_runtime_fence_tokens, packet.payload.agent_runtime_id)
       /\ mob_active_run_count' = 0
       /\ mob_member_state_markers' = MapRemove(mob_member_state_markers, packet.payload.agent_runtime_id)
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_pending_spawn_count, mob_coordinator_bound, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [kind |-> "retired"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveRuntimeRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ObserveRuntimeRetired", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_ResetMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation, arg_external_addressable) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ResetMember"
       /\ packet.payload.agent_identity = arg_agent_identity
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.generation = arg_generation
       /\ packet.payload.external_addressable = arg_external_addressable
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running" \/ mob_phase = "Stopped"
       /\ mob_phase' = "Running"
       /\ mob_live_runtime_ids' = (mob_live_runtime_ids \cup {packet.payload.agent_runtime_id})
       /\ mob_externally_addressable_runtime_ids' = IF packet.payload.external_addressable THEN (mob_externally_addressable_runtime_ids \cup {packet.payload.agent_runtime_id}) ELSE (mob_externally_addressable_runtime_ids \ {packet.payload.agent_runtime_id})
       /\ mob_runtime_fence_tokens' = MapSet(mob_runtime_fence_tokens, packet.payload.agent_runtime_id, packet.payload.fence_token)
       /\ mob_active_run_count' = 0
       /\ mob_pending_spawn_count' = 0
       /\ mob_identity_to_runtime' = MapSet(mob_identity_to_runtime, packet.payload.agent_identity, packet.payload.agent_runtime_id)
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "PrepareBindings", payload |-> [agent_runtime_id |-> packet.payload.agent_runtime_id, fence_token |-> packet.payload.fence_token, generation |-> packet.payload.generation], source_kind |-> "route", source_route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeBinding", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "PrepareBindings", payload |-> [agent_runtime_id |-> packet.payload.agent_runtime_id, fence_token |-> packet.payload.fence_token, generation |-> packet.payload.generation], source_kind |-> "route", source_route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeBinding", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", effect |-> "RequestRuntimeBinding", target_machine |-> "meerkat", target_input |-> "PrepareBindings", payload |-> [agent_runtime_id |-> packet.payload.agent_runtime_id, fence_token |-> packet.payload.fence_token, generation |-> packet.payload.generation], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "ResetMember"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestRuntimeBinding", payload |-> [agent_identity |-> packet.payload.agent_identity, agent_runtime_id |-> packet.payload.agent_runtime_id, fence_token |-> packet.payload.fence_token, generation |-> packet.payload.generation], effect_id |-> (model_step_count + 1), source_transition |-> "ResetMember"], [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [kind |-> "reset"], effect_id |-> (model_step_count + 1), source_transition |-> "ResetMember"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ResetMember", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RespawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation, arg_external_addressable) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RespawnMember"
       /\ packet.payload.agent_identity = arg_agent_identity
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.generation = arg_generation
       /\ packet.payload.external_addressable = arg_external_addressable
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_live_runtime_ids' = (mob_live_runtime_ids \cup {packet.payload.agent_runtime_id})
       /\ mob_externally_addressable_runtime_ids' = IF packet.payload.external_addressable THEN (mob_externally_addressable_runtime_ids \cup {packet.payload.agent_runtime_id}) ELSE (mob_externally_addressable_runtime_ids \ {packet.payload.agent_runtime_id})
       /\ mob_runtime_fence_tokens' = MapSet(mob_runtime_fence_tokens, packet.payload.agent_runtime_id, packet.payload.fence_token)
       /\ mob_active_run_count' = 0
       /\ mob_pending_spawn_count' = 0
       /\ mob_identity_to_runtime' = MapSet(mob_identity_to_runtime, packet.payload.agent_identity, packet.payload.agent_runtime_id)
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "PrepareBindings", payload |-> [agent_runtime_id |-> packet.payload.agent_runtime_id, fence_token |-> packet.payload.fence_token, generation |-> packet.payload.generation], source_kind |-> "route", source_route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeBinding", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "PrepareBindings", payload |-> [agent_runtime_id |-> packet.payload.agent_runtime_id, fence_token |-> packet.payload.fence_token, generation |-> packet.payload.generation], source_kind |-> "route", source_route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeBinding", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", effect |-> "RequestRuntimeBinding", target_machine |-> "meerkat", target_input |-> "PrepareBindings", payload |-> [agent_runtime_id |-> packet.payload.agent_runtime_id, fence_token |-> packet.payload.fence_token, generation |-> packet.payload.generation], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "RespawnMember"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestRuntimeBinding", payload |-> [agent_identity |-> packet.payload.agent_identity, agent_runtime_id |-> packet.payload.agent_runtime_id, fence_token |-> packet.payload.fence_token, generation |-> packet.payload.generation], effect_id |-> (model_step_count + 1), source_transition |-> "RespawnMember"], [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [kind |-> "respawned"], effect_id |-> (model_step_count + 1), source_transition |-> "RespawnMember"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RespawnMember", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_MarkCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "MarkCompleted"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running" \/ mob_phase = "Stopped"
       /\ (mob_active_run_count = 0)
       /\ mob_phase' = "Completed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [kind |-> "completed"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "MarkCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_DestroyMob ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "DestroyMob"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running" \/ mob_phase = "Stopped" \/ mob_phase = "Completed"
       /\ mob_phase' = "Destroyed"
       /\ mob_live_runtime_ids' = {}
       /\ mob_runtime_fence_tokens' = [x \in {} |-> None]
       /\ mob_active_run_count' = 0
       /\ mob_pending_spawn_count' = 0
       /\ mob_coordinator_bound' = FALSE
       /\ mob_member_state_markers' = [x \in {} |-> None]
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_externally_addressable_runtime_ids, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "Destroy", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "destroy_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeDestroy", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "Destroy", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "destroy_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeDestroy", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "destroy_request_reaches_meerkat", source_machine |-> "mob", effect |-> "RequestRuntimeDestroy", target_machine |-> "meerkat", target_input |-> "Destroy", payload |-> [tag |-> "unit"], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "DestroyMob"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestRuntimeDestroy", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "DestroyMob"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "DestroyMob", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_ObserveRuntimeDestroyed(arg_agent_runtime_id, arg_fence_token) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ObserveRuntimeDestroyed"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running" \/ mob_phase = "Stopped" \/ mob_phase = "Completed" \/ mob_phase = "Destroyed"
       /\ (packet.payload.agent_runtime_id \in mob_live_runtime_ids)
       /\ mob_phase' = "Destroyed"
       /\ mob_live_runtime_ids' = {}
       /\ mob_runtime_fence_tokens' = [x \in {} |-> None]
       /\ mob_active_run_count' = 0
       /\ mob_pending_spawn_count' = 0
       /\ mob_coordinator_bound' = FALSE
       /\ mob_member_state_markers' = [x \in {} |-> None]
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_externally_addressable_runtime_ids, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [kind |-> "destroyed"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveRuntimeDestroyed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ObserveRuntimeDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_RecordOperatorActionProvenanceRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RecordOperatorActionProvenance"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RecordOperatorActionProvenanceRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RecordOperatorActionProvenanceStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RecordOperatorActionProvenance"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RecordOperatorActionProvenanceStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_RecordOperatorActionProvenanceCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RecordOperatorActionProvenance"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Completed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RecordOperatorActionProvenanceCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_RecordOperatorActionProvenanceDestroyed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RecordOperatorActionProvenance"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Destroyed"
       /\ mob_phase' = "Destroyed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RecordOperatorActionProvenanceDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_SetSpawnPolicyRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SetSpawnPolicy"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SetSpawnPolicyRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_SetSpawnPolicyStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SetSpawnPolicy"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SetSpawnPolicyStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_SetSpawnPolicyCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SetSpawnPolicy"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Completed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SetSpawnPolicyCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_SetSpawnPolicyDestroyed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SetSpawnPolicy"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Destroyed"
       /\ mob_phase' = "Destroyed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SetSpawnPolicyDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_StopRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Stop"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_active_run_count = 0)
       /\ mob_phase' = "Stopped"
       /\ mob_active_run_count' = 0
       /\ mob_coordinator_bound' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_pending_spawn_count, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StopRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "StopRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_ResumeStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Resume"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Running"
       /\ mob_coordinator_bound' = TRUE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ResumeStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ResumeStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_CompleteRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Complete"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Completed"
       /\ mob_active_run_count' = 0
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "CompleteRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_ResetToRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Reset"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running" \/ mob_phase = "Stopped" \/ mob_phase = "Completed"
       /\ mob_phase' = "Running"
       /\ mob_active_run_count' = 0
       /\ mob_pending_spawn_count' = 0
       /\ mob_coordinator_bound' = TRUE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ResetToRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ResetToRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_WireRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Wire"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "WireRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "WireRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_ExternalTurnRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ExternalTurn"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitProgressNote", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ExternalTurnRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ExternalTurnRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_InternalTurnRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "InternalTurn"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitProgressNote", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "InternalTurnRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "InternalTurnRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_TaskCreateRunning(arg_task_id, arg_task_payload) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TaskCreate"
       /\ packet.payload.task_id = arg_task_id
       /\ packet.payload.task_payload = arg_task_payload
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ ((packet.payload.task_id \in DOMAIN mob_tasks) = FALSE)
       /\ mob_phase' = "Running"
       /\ mob_tasks' = MapSet(mob_tasks, packet.payload.task_id, packet.payload.task_payload)
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitTaskNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "TaskCreateRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TaskCreateRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_TaskUpdateRunningPending(arg_task_id, arg_new_status) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TaskUpdate"
       /\ packet.payload.task_id = arg_task_id
       /\ packet.payload.new_status = arg_new_status
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (packet.payload.new_status = "Pending")
       /\ ((packet.payload.task_id \in DOMAIN mob_tasks) = TRUE)
       /\ ((packet.payload.task_id \in mob_completed_task_ids) = FALSE)
       /\ mob_phase' = "Running"
       /\ mob_in_progress_task_ids' = (mob_in_progress_task_ids \ {packet.payload.task_id})
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitTaskNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "TaskUpdateRunningPending"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TaskUpdateRunningPending", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_TaskUpdateRunningInProgress(arg_task_id, arg_new_status) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TaskUpdate"
       /\ packet.payload.task_id = arg_task_id
       /\ packet.payload.new_status = arg_new_status
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (packet.payload.new_status = "InProgress")
       /\ ((packet.payload.task_id \in DOMAIN mob_tasks) = TRUE)
       /\ ((packet.payload.task_id \in mob_completed_task_ids) = FALSE)
       /\ mob_phase' = "Running"
       /\ mob_in_progress_task_ids' = (mob_in_progress_task_ids \cup {packet.payload.task_id})
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitTaskNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "TaskUpdateRunningInProgress"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TaskUpdateRunningInProgress", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_TaskUpdateRunningCompleted(arg_task_id, arg_new_status) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TaskUpdate"
       /\ packet.payload.task_id = arg_task_id
       /\ packet.payload.new_status = arg_new_status
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (packet.payload.new_status = "Completed")
       /\ ((packet.payload.task_id \in DOMAIN mob_tasks) = TRUE)
       /\ mob_phase' = "Running"
       /\ mob_in_progress_task_ids' = (mob_in_progress_task_ids \ {packet.payload.task_id})
       /\ mob_completed_task_ids' = (mob_completed_task_ids \cup {packet.payload.task_id})
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitTaskNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "TaskUpdateRunningCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TaskUpdateRunningCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_TaskUpdateRunningCancelled(arg_task_id, arg_new_status) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TaskUpdate"
       /\ packet.payload.task_id = arg_task_id
       /\ packet.payload.new_status = arg_new_status
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (packet.payload.new_status = "Cancelled")
       /\ ((packet.payload.task_id \in DOMAIN mob_tasks) = TRUE)
       /\ ((packet.payload.task_id \in mob_completed_task_ids) = FALSE)
       /\ mob_phase' = "Running"
       /\ mob_in_progress_task_ids' = (mob_in_progress_task_ids \ {packet.payload.task_id})
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitTaskNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "TaskUpdateRunningCancelled"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TaskUpdateRunningCancelled", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_ForceCancelRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ForceCancel"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_active_run_count' = 0
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "FlowTerminalized", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ForceCancelRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ForceCancelRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_SubscribeAgentEventsRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubscribeAgentEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_live_runtime_ids # {})
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SubscribeAgentEventsRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_SubscribeAgentEventsStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubscribeAgentEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ (mob_live_runtime_ids # {})
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SubscribeAgentEventsStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_SubscribeAgentEventsCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubscribeAgentEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ (mob_live_runtime_ids # {})
       /\ mob_phase' = "Completed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SubscribeAgentEventsCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_SubscribeAgentEventsDestroyed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubscribeAgentEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Destroyed"
       /\ (mob_live_runtime_ids # {})
       /\ mob_phase' = "Destroyed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SubscribeAgentEventsDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_SubscribeAllAgentEventsRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubscribeAllAgentEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SubscribeAllAgentEventsRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_SubscribeAllAgentEventsStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubscribeAllAgentEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SubscribeAllAgentEventsStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_SubscribeAllAgentEventsCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubscribeAllAgentEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Completed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SubscribeAllAgentEventsCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_SubscribeAllAgentEventsDestroyed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubscribeAllAgentEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Destroyed"
       /\ mob_phase' = "Destroyed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SubscribeAllAgentEventsDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_SubscribeMobEventsRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubscribeMobEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SubscribeMobEventsRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_SubscribeMobEventsStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubscribeMobEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SubscribeMobEventsStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_SubscribeMobEventsCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubscribeMobEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Completed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SubscribeMobEventsCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_SubscribeMobEventsDestroyed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubscribeMobEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Destroyed"
       /\ mob_phase' = "Destroyed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SubscribeMobEventsDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_ShutdownRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Shutdown"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Stopped"
       /\ mob_active_run_count' = 0
       /\ mob_coordinator_bound' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_pending_spawn_count, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ShutdownRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ShutdownRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_ShutdownStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Shutdown"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ mob_active_run_count' = 0
       /\ mob_coordinator_bound' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_pending_spawn_count, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ShutdownStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ShutdownStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_ShutdownCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Shutdown"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Completed"
       /\ mob_active_run_count' = 0
       /\ mob_coordinator_bound' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_pending_spawn_count, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ShutdownCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ShutdownCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_CancelFlowRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "CancelFlow"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_active_run_count' = 0
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "FlowTerminalized", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CancelFlowRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "CancelFlowRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_InitializeOrchestratorRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "InitializeOrchestrator"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_coordinator_bound' = TRUE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "InitializeOrchestratorRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "InitializeOrchestratorRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_BindCoordinatorRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "BindCoordinator"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_coordinator_bound' = TRUE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BindCoordinatorRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "BindCoordinatorRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_UnbindCoordinatorRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "UnbindCoordinator"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_coordinator_bound' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "UnbindCoordinatorRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "UnbindCoordinatorRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_StageSpawnRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "StageSpawn"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_pending_spawn_count' = (mob_pending_spawn_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "ExposePendingSpawn", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StageSpawnRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "StageSpawnRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_StopOrchestratorRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "StopOrchestrator"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_coordinator_bound' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StopOrchestratorRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "StopOrchestratorRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_StopOrchestratorStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "StopOrchestrator"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ mob_coordinator_bound' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StopOrchestratorStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "StopOrchestratorStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_StopOrchestratorCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "StopOrchestrator"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Completed"
       /\ mob_coordinator_bound' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StopOrchestratorCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "StopOrchestratorCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_ResumeOrchestratorRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ResumeOrchestrator"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_coordinator_bound' = TRUE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ResumeOrchestratorRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ResumeOrchestratorRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_ResumeOrchestratorStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ResumeOrchestrator"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ mob_coordinator_bound' = TRUE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ResumeOrchestratorStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ResumeOrchestratorStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_ResumeOrchestratorCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ResumeOrchestrator"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Completed"
       /\ mob_coordinator_bound' = TRUE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ResumeOrchestratorCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ResumeOrchestratorCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_DestroyOrchestratorRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "DestroyOrchestrator"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_coordinator_bound' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "DestroyOrchestratorRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "DestroyOrchestratorRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_DestroyOrchestratorStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "DestroyOrchestrator"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ mob_coordinator_bound' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "DestroyOrchestratorStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "DestroyOrchestratorStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_DestroyOrchestratorCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "DestroyOrchestrator"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Completed"
       /\ mob_coordinator_bound' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "DestroyOrchestratorCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "DestroyOrchestratorCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_ForceCancelMemberRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ForceCancelMember"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitMemberTerminalNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ForceCancelMemberRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ForceCancelMemberRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_MemberPeerExposedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "MemberPeerExposed"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "AdmitPeerInput", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "MemberPeerExposedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "MemberPeerExposedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_MemberTerminalizedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "MemberTerminalized"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitMemberTerminalNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "MemberTerminalizedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "MemberTerminalizedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_OperationPeerTrustedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "OperationPeerTrusted"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "AdmitPeerInput", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "OperationPeerTrustedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "OperationPeerTrustedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_PeerInputAdmittedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "PeerInputAdmitted"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "AdmitPeerInput", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PeerInputAdmittedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "PeerInputAdmittedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_BeginCleanupStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "BeginCleanup"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginCleanupStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "BeginCleanupStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_BeginCleanupCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "BeginCleanup"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginCleanupCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "BeginCleanupCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_FinishCleanupStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "FinishCleanup"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FinishCleanupStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "FinishCleanupStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_FinishCleanupCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "FinishCleanup"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FinishCleanupCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "FinishCleanupCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_RunFlowRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RunFlow"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_coordinator_bound = TRUE)
       /\ mob_phase' = "Running"
       /\ mob_active_run_count' = (mob_active_run_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitFlowRunNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RunFlowRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RunFlowRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_StartFlowRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "StartFlow"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_coordinator_bound = TRUE)
       /\ mob_phase' = "Running"
       /\ mob_active_run_count' = (mob_active_run_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitFlowRunNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StartFlowRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "StartFlowRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_CreateRunRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "CreateRun"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_active_run_count' = (mob_active_run_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CreateRunRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "CreateRunRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_StartRunRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "StartRun"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_active_run_count' = (mob_active_run_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StartRunRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "StartRunRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_UnwireRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Unwire"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "UnwireRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "UnwireRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_CompleteFlowRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "CompleteFlow"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running" \/ mob_phase = "Completed"
       /\ (mob_active_run_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_active_run_count' = (mob_active_run_count) - 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "FlowTerminalized", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteFlowRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "CompleteFlowRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_FinishRunRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "FinishRun"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running" \/ mob_phase = "Stopped"
       /\ (mob_active_run_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_active_run_count' = (mob_active_run_count) - 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FinishRunRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "FinishRunRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RetireRunning(arg_agent_runtime_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Retire"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_live_runtime_ids # {})
       /\ (packet.payload.agent_runtime_id \in mob_live_runtime_ids)
       /\ mob_phase' = "Running"
       /\ mob_member_state_markers' = MapSet(mob_member_state_markers, packet.payload.agent_runtime_id, "Retiring")
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "Retire", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeRetire", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "Retire", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeRetire", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", effect |-> "RequestRuntimeRetire", target_machine |-> "meerkat", target_input |-> "Retire", payload |-> [tag |-> "unit"], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "RetireRunning"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestRuntimeRetire", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RetireRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RetireRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RetireStopped(arg_agent_runtime_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Retire"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ (mob_live_runtime_ids # {})
       /\ (packet.payload.agent_runtime_id \in mob_live_runtime_ids)
       /\ mob_phase' = "Stopped"
       /\ mob_member_state_markers' = MapSet(mob_member_state_markers, packet.payload.agent_runtime_id, "Retiring")
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "Retire", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeRetire", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "Retire", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeRetire", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", effect |-> "RequestRuntimeRetire", target_machine |-> "meerkat", target_input |-> "Retire", payload |-> [tag |-> "unit"], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "RetireStopped"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestRuntimeRetire", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RetireStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RetireStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_RetireAllRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RetireAll"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_live_runtime_ids' = {}
       /\ mob_runtime_fence_tokens' = [x \in {} |-> None]
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_externally_addressable_runtime_ids, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [kind |-> "retiring"], effect_id |-> (model_step_count + 1), source_transition |-> "RetireAllRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RetireAllRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RetireAllStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RetireAll"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ mob_live_runtime_ids' = {}
       /\ mob_runtime_fence_tokens' = [x \in {} |-> None]
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_externally_addressable_runtime_ids, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [kind |-> "retiring"], effect_id |-> (model_step_count + 1), source_transition |-> "RetireAllStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RetireAllStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_CompleteSpawnRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "CompleteSpawn"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running" \/ mob_phase = "Stopped"
       /\ (mob_pending_spawn_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_pending_spawn_count' = (mob_pending_spawn_count) - 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [kind |-> "spawned"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteSpawnRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "CompleteSpawnRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_DestroyFromAny ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Destroy"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running" \/ mob_phase = "Stopped" \/ mob_phase = "Completed"
       /\ mob_phase' = "Destroyed"
       /\ mob_live_runtime_ids' = {}
       /\ mob_runtime_fence_tokens' = [x \in {} |-> None]
       /\ mob_active_run_count' = 0
       /\ mob_pending_spawn_count' = 0
       /\ mob_coordinator_bound' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_externally_addressable_runtime_ids, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "DestroyFromAny", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_RespawnRunning(arg_agent_runtime_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Respawn"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (packet.payload.agent_runtime_id \in mob_live_runtime_ids)
       /\ (mob_coordinator_bound = TRUE)
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "ExposePendingSpawn", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RespawnRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RespawnRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_CancelAllWorkRunning(arg_agent_runtime_id, arg_fence_token) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "CancelAllWork"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_live_runtime_ids # {})
       /\ (packet.payload.agent_runtime_id \in mob_live_runtime_ids)
       /\ mob_phase' = "Running"
       /\ mob_active_run_count' = 0
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "FlowTerminalized", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CancelAllWorkRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "CancelAllWorkRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


EntryPacketAdmissible_meerkat(packet) ==
    \/ /\ (packet.variant = "RegisterSession") /\ (meerkat_phase = "Idle")
    \/ /\ (packet.variant = "RegisterSession") /\ (meerkat_phase = "Attached")
    \/ /\ (packet.variant = "RegisterSession") /\ (meerkat_phase = "Running")
    \/ /\ (packet.variant = "RegisterSession") /\ (meerkat_phase = "Retired")
    \/ /\ (packet.variant = "RegisterSession") /\ (meerkat_phase = "Stopped")
    \/ /\ (packet.variant = "UnregisterSession") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id = Some(packet.payload.session_id)))
    \/ /\ (packet.variant = "UnregisterSession") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id = Some(packet.payload.session_id)))
    \/ /\ (packet.variant = "UnregisterSession") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id = Some(packet.payload.session_id)))
    \/ /\ (packet.variant = "UnregisterSession") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id = Some(packet.payload.session_id)))
    \/ /\ (packet.variant = "UnregisterSession") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id = Some(packet.payload.session_id)))
    \/ /\ (packet.variant = "ReconfigureSessionLlmIdentity") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None)) /\ ((meerkat_active_runtime_id # None))
    \/ /\ (packet.variant = "ReconfigureSessionLlmIdentity") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None)) /\ ((meerkat_active_runtime_id # None))
    \/ /\ (packet.variant = "StagePersistentFilter") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "StagePersistentFilter") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "StagePersistentFilter") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "StagePersistentFilter") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "StagePersistentFilter") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "RequestDeferredTools") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "RequestDeferredTools") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "RequestDeferredTools") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "RequestDeferredTools") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "RequestDeferredTools") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "PrepareBindings") /\ (meerkat_phase = "Initializing")
    \/ /\ (packet.variant = "PrepareBindings") /\ (meerkat_phase = "Idle")
    \/ /\ (packet.variant = "PrepareBindings") /\ (meerkat_phase = "Attached")
    \/ /\ (packet.variant = "PrepareBindings") /\ (meerkat_phase = "Running")
    \/ /\ (packet.variant = "PrepareBindings") /\ (meerkat_phase = "Retired")
    \/ /\ (packet.variant = "PrepareBindings") /\ (meerkat_phase = "Stopped")
    \/ /\ (packet.variant = "SetPeerIngressContext") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "SetPeerIngressContext") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "SetPeerIngressContext") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "SetPeerIngressContext") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "SetPeerIngressContext") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "NotifyDrainExited") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "NotifyDrainExited") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "NotifyDrainExited") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "NotifyDrainExited") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "NotifyDrainExited") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "InterruptCurrentRun") /\ (meerkat_phase = "Attached")
    \/ /\ (packet.variant = "InterruptCurrentRun") /\ (meerkat_phase = "Running")
    \/ /\ (packet.variant = "CancelAfterBoundary") /\ (meerkat_phase = "Attached")
    \/ /\ (packet.variant = "CancelAfterBoundary") /\ (meerkat_phase = "Running")
    \/ /\ (packet.variant = "PublishCommittedVisibleSet") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None)) /\ ((packet.payload.active_visibility_revision >= packet.payload.staged_visibility_revision)) /\ (((packet.payload.active_visibility_revision # packet.payload.staged_visibility_revision) \/ ((packet.payload.active_filter = packet.payload.staged_filter) /\ (packet.payload.active_requested_deferred_names = packet.payload.staged_requested_deferred_names)))) /\ ((\A requested_name \in packet.payload.active_requested_deferred_names : (requested_name \in packet.payload.staged_requested_deferred_names)))
    \/ /\ (packet.variant = "PublishCommittedVisibleSet") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None)) /\ ((packet.payload.active_visibility_revision >= packet.payload.staged_visibility_revision)) /\ (((packet.payload.active_visibility_revision # packet.payload.staged_visibility_revision) \/ ((packet.payload.active_filter = packet.payload.staged_filter) /\ (packet.payload.active_requested_deferred_names = packet.payload.staged_requested_deferred_names)))) /\ ((\A requested_name \in packet.payload.active_requested_deferred_names : (requested_name \in packet.payload.staged_requested_deferred_names)))
    \/ /\ (packet.variant = "PublishCommittedVisibleSet") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None)) /\ ((packet.payload.active_visibility_revision >= packet.payload.staged_visibility_revision)) /\ (((packet.payload.active_visibility_revision # packet.payload.staged_visibility_revision) \/ ((packet.payload.active_filter = packet.payload.staged_filter) /\ (packet.payload.active_requested_deferred_names = packet.payload.staged_requested_deferred_names)))) /\ ((\A requested_name \in packet.payload.active_requested_deferred_names : (requested_name \in packet.payload.staged_requested_deferred_names)))
    \/ /\ (packet.variant = "PublishCommittedVisibleSet") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None)) /\ ((packet.payload.active_visibility_revision >= packet.payload.staged_visibility_revision)) /\ (((packet.payload.active_visibility_revision # packet.payload.staged_visibility_revision) \/ ((packet.payload.active_filter = packet.payload.staged_filter) /\ (packet.payload.active_requested_deferred_names = packet.payload.staged_requested_deferred_names)))) /\ ((\A requested_name \in packet.payload.active_requested_deferred_names : (requested_name \in packet.payload.staged_requested_deferred_names)))
    \/ /\ (packet.variant = "PublishCommittedVisibleSet") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None)) /\ ((packet.payload.active_visibility_revision >= packet.payload.staged_visibility_revision)) /\ (((packet.payload.active_visibility_revision # packet.payload.staged_visibility_revision) \/ ((packet.payload.active_filter = packet.payload.staged_filter) /\ (packet.payload.active_requested_deferred_names = packet.payload.staged_requested_deferred_names)))) /\ ((\A requested_name \in packet.payload.active_requested_deferred_names : (requested_name \in packet.payload.staged_requested_deferred_names)))
    \/ /\ (packet.variant = "Retire") /\ (meerkat_phase = "Idle" \/ meerkat_phase = "Attached" \/ meerkat_phase = "Running")
    \/ /\ (packet.variant = "Reset") /\ (meerkat_phase = "Initializing" \/ meerkat_phase = "Idle" \/ meerkat_phase = "Attached" \/ meerkat_phase = "Retired")
    \/ /\ (packet.variant = "StopRuntimeExecutor") /\ (meerkat_phase = "Initializing" \/ meerkat_phase = "Idle" \/ meerkat_phase = "Retired")
    \/ /\ (packet.variant = "StopRuntimeExecutor") /\ (meerkat_phase = "Attached")
    \/ /\ (packet.variant = "StopRuntimeExecutor") /\ (meerkat_phase = "Running")
    \/ /\ (packet.variant = "Destroy") /\ (meerkat_phase = "Initializing" \/ meerkat_phase = "Idle" \/ meerkat_phase = "Attached" \/ meerkat_phase = "Running" \/ meerkat_phase = "Retired" \/ meerkat_phase = "Stopped") /\ ((meerkat_active_runtime_id # None))
    \/ /\ (packet.variant = "EnsureSessionWithExecutor") /\ (meerkat_phase = "Idle")
    \/ /\ (packet.variant = "EnsureSessionWithExecutor") /\ (meerkat_phase = "Attached")
    \/ /\ (packet.variant = "EnsureSessionWithExecutor") /\ (meerkat_phase = "Running")
    \/ /\ (packet.variant = "EnsureSessionWithExecutor") /\ (meerkat_phase = "Retired")
    \/ /\ (packet.variant = "EnsureSessionWithExecutor") /\ (meerkat_phase = "Stopped")
    \/ /\ (packet.variant = "SetSilentIntents") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "SetSilentIntents") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "SetSilentIntents") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "SetSilentIntents") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "SetSilentIntents") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "Abort") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "Abort") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "Abort") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "Abort") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "Abort") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "Wait") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "Wait") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "Wait") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "Wait") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "Wait") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "AbortAll") /\ (meerkat_phase = "Idle")
    \/ /\ (packet.variant = "AbortAll") /\ (meerkat_phase = "Attached")
    \/ /\ (packet.variant = "AbortAll") /\ (meerkat_phase = "Running")
    \/ /\ (packet.variant = "AbortAll") /\ (meerkat_phase = "Retired")
    \/ /\ (packet.variant = "AbortAll") /\ (meerkat_phase = "Stopped")
    \/ /\ (packet.variant = "Ingest") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "Ingest") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "Ingest") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "PublishEvent") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "PublishEvent") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "PublishEvent") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "PublishEvent") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "PublishEvent") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "AcceptWithCompletion") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None)) /\ ((packet.payload.request_immediate_processing = FALSE)) /\ ((packet.payload.interrupt_yielding = FALSE))
    \/ /\ (packet.variant = "AcceptWithCompletion") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None)) /\ ((packet.payload.request_immediate_processing = TRUE)) /\ ((packet.payload.interrupt_yielding = FALSE))
    \/ /\ (packet.variant = "AcceptWithCompletion") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None)) /\ ((packet.payload.request_immediate_processing = TRUE)) /\ ((packet.payload.interrupt_yielding = FALSE))
    \/ /\ (packet.variant = "AcceptWithCompletion") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None)) /\ ((packet.payload.request_immediate_processing = FALSE)) /\ ((packet.payload.interrupt_yielding = FALSE))
    \/ /\ (packet.variant = "AcceptWithCompletion") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None)) /\ ((packet.payload.request_immediate_processing = FALSE)) /\ ((packet.payload.interrupt_yielding = FALSE)) /\ ((packet.payload.wake_if_idle = FALSE))
    \/ /\ (packet.variant = "AcceptWithCompletion") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None)) /\ ((packet.payload.request_immediate_processing = FALSE)) /\ ((packet.payload.interrupt_yielding = FALSE)) /\ ((packet.payload.wake_if_idle = TRUE))
    \/ /\ (packet.variant = "AcceptWithCompletion") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None)) /\ ((packet.payload.request_immediate_processing = FALSE)) /\ ((packet.payload.interrupt_yielding = TRUE))
    \/ /\ (packet.variant = "AcceptWithCompletion") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None)) /\ ((packet.payload.request_immediate_processing = TRUE)) /\ ((packet.payload.interrupt_yielding = FALSE))
    \/ /\ (packet.variant = "AcceptWithoutWake") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "AcceptWithoutWake") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "AcceptWithoutWake") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "Prepare") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "Prepare") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "Commit") /\ (meerkat_phase = "Running") /\ ((meerkat_pre_run_phase = Some("idle"))) /\ ((meerkat_current_run_id = Some(packet.payload.run_id)))
    \/ /\ (packet.variant = "Commit") /\ (meerkat_phase = "Running") /\ ((meerkat_pre_run_phase = Some("attached"))) /\ ((meerkat_current_run_id = Some(packet.payload.run_id)))
    \/ /\ (packet.variant = "Commit") /\ (meerkat_phase = "Running") /\ ((meerkat_pre_run_phase = Some("retired"))) /\ ((meerkat_current_run_id = Some(packet.payload.run_id)))
    \/ /\ (packet.variant = "Fail") /\ (meerkat_phase = "Running") /\ ((meerkat_pre_run_phase = Some("idle"))) /\ ((meerkat_current_run_id = Some(packet.payload.run_id)))
    \/ /\ (packet.variant = "Fail") /\ (meerkat_phase = "Running") /\ ((meerkat_pre_run_phase = Some("attached"))) /\ ((meerkat_current_run_id = Some(packet.payload.run_id)))
    \/ /\ (packet.variant = "Fail") /\ (meerkat_phase = "Running") /\ ((meerkat_pre_run_phase = Some("retired"))) /\ ((meerkat_current_run_id = Some(packet.payload.run_id)))
    \/ /\ (packet.variant = "Recycle") /\ (meerkat_phase = "Idle" \/ meerkat_phase = "Retired") /\ ((meerkat_active_runtime_id # None))
    \/ /\ (packet.variant = "Recycle") /\ (meerkat_phase = "Attached") /\ ((meerkat_active_runtime_id # None))
    \/ /\ (packet.variant = "ProjectRealtimeIntent") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "ProjectRealtimeIntent") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "ProjectRealtimeIntent") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "ProjectRealtimeIntent") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "ProjectRealtimeIntent") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "BeginRealtimeBinding") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Idle"))
    \/ /\ (packet.variant = "BeginRealtimeBinding") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Idle"))
    \/ /\ (packet.variant = "BeginRealtimeBinding") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Idle"))
    \/ /\ (packet.variant = "BeginRealtimeBinding") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Idle"))
    \/ /\ (packet.variant = "BeginRealtimeBinding") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Idle"))
    \/ /\ (packet.variant = "ReplaceRealtimeBinding") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Idle"))
    \/ /\ (packet.variant = "ReplaceRealtimeBinding") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Idle"))
    \/ /\ (packet.variant = "ReplaceRealtimeBinding") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Idle"))
    \/ /\ (packet.variant = "ReplaceRealtimeBinding") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Idle"))
    \/ /\ (packet.variant = "ReplaceRealtimeBinding") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Idle"))
    \/ /\ (packet.variant = "DetachRealtimeBinding") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "DetachRealtimeBinding") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "DetachRealtimeBinding") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "DetachRealtimeBinding") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "DetachRealtimeBinding") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "RequireRealtimeReattach") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "RequireRealtimeReattach") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "RequireRealtimeReattach") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "RequireRealtimeReattach") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "RequireRealtimeReattach") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "PublishRealtimeSignal") /\ (meerkat_phase = "Idle") /\ ((meerkat_realtime_binding_authority_epoch = Some(packet.payload.authority_epoch))) /\ ((meerkat_live_topology_phase = "Idle")) /\ (((packet.payload.next_binding_state = "BindingNotReady") \/ (packet.payload.next_binding_state = "BindingReady") \/ (packet.payload.next_binding_state = "ReplacementPending")))
    \/ /\ (packet.variant = "PublishRealtimeSignal") /\ (meerkat_phase = "Attached") /\ ((meerkat_realtime_binding_authority_epoch = Some(packet.payload.authority_epoch))) /\ ((meerkat_live_topology_phase = "Idle")) /\ (((packet.payload.next_binding_state = "BindingNotReady") \/ (packet.payload.next_binding_state = "BindingReady") \/ (packet.payload.next_binding_state = "ReplacementPending")))
    \/ /\ (packet.variant = "PublishRealtimeSignal") /\ (meerkat_phase = "Running") /\ ((meerkat_realtime_binding_authority_epoch = Some(packet.payload.authority_epoch))) /\ ((meerkat_live_topology_phase = "Idle")) /\ (((packet.payload.next_binding_state = "BindingNotReady") \/ (packet.payload.next_binding_state = "BindingReady") \/ (packet.payload.next_binding_state = "ReplacementPending")))
    \/ /\ (packet.variant = "PublishRealtimeSignal") /\ (meerkat_phase = "Retired") /\ ((meerkat_realtime_binding_authority_epoch = Some(packet.payload.authority_epoch))) /\ ((meerkat_live_topology_phase = "Idle")) /\ (((packet.payload.next_binding_state = "BindingNotReady") \/ (packet.payload.next_binding_state = "BindingReady") \/ (packet.payload.next_binding_state = "ReplacementPending")))
    \/ /\ (packet.variant = "PublishRealtimeSignal") /\ (meerkat_phase = "Stopped") /\ ((meerkat_realtime_binding_authority_epoch = Some(packet.payload.authority_epoch))) /\ ((meerkat_live_topology_phase = "Idle")) /\ (((packet.payload.next_binding_state = "BindingNotReady") \/ (packet.payload.next_binding_state = "BindingReady") \/ (packet.payload.next_binding_state = "ReplacementPending")))
    \/ /\ (packet.variant = "McpServerConnectPending") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerConnectPending") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerConnectPending") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerConnectPending") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerConnectPending") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerConnected") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerConnected") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerConnected") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerConnected") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerConnected") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerFailed") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerFailed") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerFailed") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerFailed") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerFailed") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerDisconnected") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerDisconnected") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerDisconnected") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerDisconnected") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerDisconnected") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerReload") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerReload") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerReload") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerReload") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "McpServerReload") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None))
    \/ /\ (packet.variant = "BeginLiveTopologyReconfigure") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None)) /\ ((meerkat_realtime_binding_authority_epoch = Some(packet.payload.authority_epoch))) /\ ((meerkat_live_topology_phase = "Idle"))
    \/ /\ (packet.variant = "BeginLiveTopologyReconfigure") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None)) /\ ((meerkat_realtime_binding_authority_epoch = Some(packet.payload.authority_epoch))) /\ ((meerkat_live_topology_phase = "Idle"))
    \/ /\ (packet.variant = "BeginLiveTopologyReconfigure") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None)) /\ ((meerkat_realtime_binding_authority_epoch = Some(packet.payload.authority_epoch))) /\ ((meerkat_live_topology_phase = "Idle"))
    \/ /\ (packet.variant = "BeginLiveTopologyReconfigure") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None)) /\ ((meerkat_realtime_binding_authority_epoch = Some(packet.payload.authority_epoch))) /\ ((meerkat_live_topology_phase = "Idle"))
    \/ /\ (packet.variant = "BeginLiveTopologyReconfigure") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None)) /\ ((meerkat_realtime_binding_authority_epoch = Some(packet.payload.authority_epoch))) /\ ((meerkat_live_topology_phase = "Idle"))
    \/ /\ (packet.variant = "MarkLiveTopologyDetached") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Reconfiguring")) /\ ((meerkat_current_run_id = None))
    \/ /\ (packet.variant = "MarkLiveTopologyDetached") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Reconfiguring")) /\ ((meerkat_current_run_id = None))
    \/ /\ (packet.variant = "MarkLiveTopologyDetached") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Reconfiguring")) /\ ((meerkat_current_run_id = None))
    \/ /\ (packet.variant = "MarkLiveTopologyDetached") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Reconfiguring")) /\ ((meerkat_current_run_id = None))
    \/ /\ (packet.variant = "MarkLiveTopologyDetached") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Reconfiguring")) /\ ((meerkat_current_run_id = None))
    \/ /\ (packet.variant = "ApplyLiveTopologyIdentity") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Detached"))
    \/ /\ (packet.variant = "ApplyLiveTopologyIdentity") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Detached"))
    \/ /\ (packet.variant = "ApplyLiveTopologyIdentity") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Detached"))
    \/ /\ (packet.variant = "ApplyLiveTopologyIdentity") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Detached"))
    \/ /\ (packet.variant = "ApplyLiveTopologyIdentity") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Detached"))
    \/ /\ (packet.variant = "ApplyLiveTopologyVisibility") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "HostIdentityApplied"))
    \/ /\ (packet.variant = "ApplyLiveTopologyVisibility") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "HostIdentityApplied"))
    \/ /\ (packet.variant = "ApplyLiveTopologyVisibility") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "HostIdentityApplied"))
    \/ /\ (packet.variant = "ApplyLiveTopologyVisibility") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "HostIdentityApplied"))
    \/ /\ (packet.variant = "ApplyLiveTopologyVisibility") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "HostIdentityApplied"))
    \/ /\ (packet.variant = "CompleteLiveTopology") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "HostVisibilityApplied"))
    \/ /\ (packet.variant = "CompleteLiveTopology") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "HostVisibilityApplied"))
    \/ /\ (packet.variant = "CompleteLiveTopology") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "HostVisibilityApplied"))
    \/ /\ (packet.variant = "CompleteLiveTopology") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "HostVisibilityApplied"))
    \/ /\ (packet.variant = "CompleteLiveTopology") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "HostVisibilityApplied"))
    \/ /\ (packet.variant = "AbortLiveTopologyBeforeDetach") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Reconfiguring"))
    \/ /\ (packet.variant = "AbortLiveTopologyBeforeDetach") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Reconfiguring"))
    \/ /\ (packet.variant = "AbortLiveTopologyBeforeDetach") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Reconfiguring"))
    \/ /\ (packet.variant = "AbortLiveTopologyBeforeDetach") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Reconfiguring"))
    \/ /\ (packet.variant = "AbortLiveTopologyBeforeDetach") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None)) /\ ((meerkat_live_topology_phase = "Reconfiguring"))
    \/ /\ (packet.variant = "FailLiveTopologyAfterDetach") /\ (meerkat_phase = "Idle") /\ ((meerkat_session_id # None)) /\ (((meerkat_live_topology_phase = "Detached") \/ (meerkat_live_topology_phase = "HostIdentityApplied") \/ (meerkat_live_topology_phase = "HostVisibilityApplied")))
    \/ /\ (packet.variant = "FailLiveTopologyAfterDetach") /\ (meerkat_phase = "Attached") /\ ((meerkat_session_id # None)) /\ (((meerkat_live_topology_phase = "Detached") \/ (meerkat_live_topology_phase = "HostIdentityApplied") \/ (meerkat_live_topology_phase = "HostVisibilityApplied")))
    \/ /\ (packet.variant = "FailLiveTopologyAfterDetach") /\ (meerkat_phase = "Running") /\ ((meerkat_session_id # None)) /\ (((meerkat_live_topology_phase = "Detached") \/ (meerkat_live_topology_phase = "HostIdentityApplied") \/ (meerkat_live_topology_phase = "HostVisibilityApplied")))
    \/ /\ (packet.variant = "FailLiveTopologyAfterDetach") /\ (meerkat_phase = "Retired") /\ ((meerkat_session_id # None)) /\ (((meerkat_live_topology_phase = "Detached") \/ (meerkat_live_topology_phase = "HostIdentityApplied") \/ (meerkat_live_topology_phase = "HostVisibilityApplied")))
    \/ /\ (packet.variant = "FailLiveTopologyAfterDetach") /\ (meerkat_phase = "Stopped") /\ ((meerkat_session_id # None)) /\ (((meerkat_live_topology_phase = "Detached") \/ (meerkat_live_topology_phase = "HostIdentityApplied") \/ (meerkat_live_topology_phase = "HostVisibilityApplied")))

EntryPacketAdmissible_mob(packet) ==
    \/ /\ (packet.variant = "Spawn") /\ (mob_phase = "Running") /\ ((mob_coordinator_bound = TRUE))
    \/ /\ (packet.variant = "SubmitWork") /\ (mob_phase = "Running") /\ ((mob_live_runtime_ids # {})) /\ ((packet.payload.agent_runtime_id \in mob_live_runtime_ids)) /\ ((packet.payload.origin = "External")) /\ ((packet.payload.agent_runtime_id \in mob_externally_addressable_runtime_ids))
    \/ /\ (packet.variant = "SubmitWork") /\ (mob_phase = "Running") /\ ((mob_live_runtime_ids # {})) /\ ((packet.payload.agent_runtime_id \in mob_live_runtime_ids)) /\ ((packet.payload.origin = "Internal"))
    \/ /\ (packet.variant = "RecordOperatorActionProvenance") /\ (mob_phase = "Running")
    \/ /\ (packet.variant = "RecordOperatorActionProvenance") /\ (mob_phase = "Stopped")
    \/ /\ (packet.variant = "RecordOperatorActionProvenance") /\ (mob_phase = "Completed")
    \/ /\ (packet.variant = "RecordOperatorActionProvenance") /\ (mob_phase = "Destroyed")
    \/ /\ (packet.variant = "SetSpawnPolicy") /\ (mob_phase = "Running")
    \/ /\ (packet.variant = "SetSpawnPolicy") /\ (mob_phase = "Stopped")
    \/ /\ (packet.variant = "SetSpawnPolicy") /\ (mob_phase = "Completed")
    \/ /\ (packet.variant = "SetSpawnPolicy") /\ (mob_phase = "Destroyed")
    \/ /\ (packet.variant = "Stop") /\ (mob_phase = "Running") /\ ((mob_active_run_count = 0))
    \/ /\ (packet.variant = "Resume") /\ (mob_phase = "Stopped")
    \/ /\ (packet.variant = "Complete") /\ (mob_phase = "Running")
    \/ /\ (packet.variant = "Reset") /\ (mob_phase = "Running" \/ mob_phase = "Stopped" \/ mob_phase = "Completed")
    \/ /\ (packet.variant = "Wire") /\ (mob_phase = "Running")
    \/ /\ (packet.variant = "ExternalTurn") /\ (mob_phase = "Running")
    \/ /\ (packet.variant = "InternalTurn") /\ (mob_phase = "Running")
    \/ /\ (packet.variant = "TaskCreate") /\ (mob_phase = "Running") /\ (((packet.payload.task_id \in DOMAIN mob_tasks) = FALSE))
    \/ /\ (packet.variant = "TaskUpdate") /\ (mob_phase = "Running") /\ ((packet.payload.new_status = "Pending")) /\ (((packet.payload.task_id \in DOMAIN mob_tasks) = TRUE)) /\ (((packet.payload.task_id \in mob_completed_task_ids) = FALSE))
    \/ /\ (packet.variant = "TaskUpdate") /\ (mob_phase = "Running") /\ ((packet.payload.new_status = "InProgress")) /\ (((packet.payload.task_id \in DOMAIN mob_tasks) = TRUE)) /\ (((packet.payload.task_id \in mob_completed_task_ids) = FALSE))
    \/ /\ (packet.variant = "TaskUpdate") /\ (mob_phase = "Running") /\ ((packet.payload.new_status = "Completed")) /\ (((packet.payload.task_id \in DOMAIN mob_tasks) = TRUE))
    \/ /\ (packet.variant = "TaskUpdate") /\ (mob_phase = "Running") /\ ((packet.payload.new_status = "Cancelled")) /\ (((packet.payload.task_id \in DOMAIN mob_tasks) = TRUE)) /\ (((packet.payload.task_id \in mob_completed_task_ids) = FALSE))
    \/ /\ (packet.variant = "ForceCancel") /\ (mob_phase = "Running")
    \/ /\ (packet.variant = "SubscribeAgentEvents") /\ (mob_phase = "Running") /\ ((mob_live_runtime_ids # {}))
    \/ /\ (packet.variant = "SubscribeAgentEvents") /\ (mob_phase = "Stopped") /\ ((mob_live_runtime_ids # {}))
    \/ /\ (packet.variant = "SubscribeAgentEvents") /\ (mob_phase = "Completed") /\ ((mob_live_runtime_ids # {}))
    \/ /\ (packet.variant = "SubscribeAgentEvents") /\ (mob_phase = "Destroyed") /\ ((mob_live_runtime_ids # {}))
    \/ /\ (packet.variant = "SubscribeAllAgentEvents") /\ (mob_phase = "Running")
    \/ /\ (packet.variant = "SubscribeAllAgentEvents") /\ (mob_phase = "Stopped")
    \/ /\ (packet.variant = "SubscribeAllAgentEvents") /\ (mob_phase = "Completed")
    \/ /\ (packet.variant = "SubscribeAllAgentEvents") /\ (mob_phase = "Destroyed")
    \/ /\ (packet.variant = "SubscribeMobEvents") /\ (mob_phase = "Running")
    \/ /\ (packet.variant = "SubscribeMobEvents") /\ (mob_phase = "Stopped")
    \/ /\ (packet.variant = "SubscribeMobEvents") /\ (mob_phase = "Completed")
    \/ /\ (packet.variant = "SubscribeMobEvents") /\ (mob_phase = "Destroyed")
    \/ /\ (packet.variant = "Shutdown") /\ (mob_phase = "Running")
    \/ /\ (packet.variant = "Shutdown") /\ (mob_phase = "Stopped")
    \/ /\ (packet.variant = "Shutdown") /\ (mob_phase = "Completed")
    \/ /\ (packet.variant = "CancelFlow") /\ (mob_phase = "Running")
    \/ /\ (packet.variant = "RunFlow") /\ (mob_phase = "Running") /\ ((mob_coordinator_bound = TRUE))
    \/ /\ (packet.variant = "Unwire") /\ (mob_phase = "Running")
    \/ /\ (packet.variant = "Retire") /\ (mob_phase = "Running") /\ ((mob_live_runtime_ids # {})) /\ ((packet.payload.agent_runtime_id \in mob_live_runtime_ids))
    \/ /\ (packet.variant = "Retire") /\ (mob_phase = "Stopped") /\ ((mob_live_runtime_ids # {})) /\ ((packet.payload.agent_runtime_id \in mob_live_runtime_ids))
    \/ /\ (packet.variant = "RetireAll") /\ (mob_phase = "Running")
    \/ /\ (packet.variant = "RetireAll") /\ (mob_phase = "Stopped")
    \/ /\ (packet.variant = "Destroy") /\ (mob_phase = "Running" \/ mob_phase = "Stopped" \/ mob_phase = "Completed")
    \/ /\ (packet.variant = "Respawn") /\ (mob_phase = "Running") /\ ((packet.payload.agent_runtime_id \in mob_live_runtime_ids)) /\ ((mob_coordinator_bound = TRUE))
    \/ /\ (packet.variant = "CancelAllWork") /\ (mob_phase = "Running") /\ ((mob_live_runtime_ids # {})) /\ ((packet.payload.agent_runtime_id \in mob_live_runtime_ids))

EntryPacketAdmissible(packet) ==
    CASE
      packet.machine = "meerkat" -> EntryPacketAdmissible_meerkat(packet)
      [] packet.machine = "mob" -> EntryPacketAdmissible_mob(packet)
      [] OTHER -> FALSE

Inject_spawn_member(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation, arg_external_addressable) ==
    /\ ~([machine |-> "mob", variant |-> "Spawn", payload |-> [agent_identity |-> arg_agent_identity, agent_runtime_id |-> arg_agent_runtime_id, external_addressable |-> arg_external_addressable, fence_token |-> arg_fence_token, generation |-> arg_generation], source_kind |-> "entry", source_route |-> "spawn_member", source_machine |-> "external_entry", source_effect |-> "Spawn", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ EntryPacketAdmissible([machine |-> "mob", variant |-> "Spawn", payload |-> [agent_identity |-> arg_agent_identity, agent_runtime_id |-> arg_agent_runtime_id, external_addressable |-> arg_external_addressable, fence_token |-> arg_fence_token, generation |-> arg_generation], source_kind |-> "entry", source_route |-> "spawn_member", source_machine |-> "external_entry", source_effect |-> "Spawn", effect_id |-> 0])
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "mob", variant |-> "Spawn", payload |-> [agent_identity |-> arg_agent_identity, agent_runtime_id |-> arg_agent_runtime_id, external_addressable |-> arg_external_addressable, fence_token |-> arg_fence_token, generation |-> arg_generation], source_kind |-> "entry", source_route |-> "spawn_member", source_machine |-> "external_entry", source_effect |-> "Spawn", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "Spawn", payload |-> [agent_identity |-> arg_agent_identity, agent_runtime_id |-> arg_agent_runtime_id, external_addressable |-> arg_external_addressable, fence_token |-> arg_fence_token, generation |-> arg_generation], source_kind |-> "entry", source_route |-> "spawn_member", source_machine |-> "external_entry", source_effect |-> "Spawn", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_submit_work(arg_agent_runtime_id, arg_fence_token, arg_work_id, arg_origin) ==
    /\ ~([machine |-> "mob", variant |-> "SubmitWork", payload |-> [agent_runtime_id |-> arg_agent_runtime_id, fence_token |-> arg_fence_token, origin |-> arg_origin, work_id |-> arg_work_id], source_kind |-> "entry", source_route |-> "submit_work", source_machine |-> "external_entry", source_effect |-> "SubmitWork", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ EntryPacketAdmissible([machine |-> "mob", variant |-> "SubmitWork", payload |-> [agent_runtime_id |-> arg_agent_runtime_id, fence_token |-> arg_fence_token, origin |-> arg_origin, work_id |-> arg_work_id], source_kind |-> "entry", source_route |-> "submit_work", source_machine |-> "external_entry", source_effect |-> "SubmitWork", effect_id |-> 0])
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "mob", variant |-> "SubmitWork", payload |-> [agent_runtime_id |-> arg_agent_runtime_id, fence_token |-> arg_fence_token, origin |-> arg_origin, work_id |-> arg_work_id], source_kind |-> "entry", source_route |-> "submit_work", source_machine |-> "external_entry", source_effect |-> "SubmitWork", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "SubmitWork", payload |-> [agent_runtime_id |-> arg_agent_runtime_id, fence_token |-> arg_fence_token, origin |-> arg_origin, work_id |-> arg_work_id], source_kind |-> "entry", source_route |-> "submit_work", source_machine |-> "external_entry", source_effect |-> "SubmitWork", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_retire_member(arg_agent_runtime_id) ==
    /\ ~([machine |-> "mob", variant |-> "Retire", payload |-> [agent_runtime_id |-> arg_agent_runtime_id], source_kind |-> "entry", source_route |-> "retire_member", source_machine |-> "external_entry", source_effect |-> "Retire", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ EntryPacketAdmissible([machine |-> "mob", variant |-> "Retire", payload |-> [agent_runtime_id |-> arg_agent_runtime_id], source_kind |-> "entry", source_route |-> "retire_member", source_machine |-> "external_entry", source_effect |-> "Retire", effect_id |-> 0])
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "mob", variant |-> "Retire", payload |-> [agent_runtime_id |-> arg_agent_runtime_id], source_kind |-> "entry", source_route |-> "retire_member", source_machine |-> "external_entry", source_effect |-> "Retire", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "Retire", payload |-> [agent_runtime_id |-> arg_agent_runtime_id], source_kind |-> "entry", source_route |-> "retire_member", source_machine |-> "external_entry", source_effect |-> "Retire", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_destroy_mob ==
    /\ ~([machine |-> "mob", variant |-> "Destroy", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "destroy_mob", source_machine |-> "external_entry", source_effect |-> "Destroy", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ EntryPacketAdmissible([machine |-> "mob", variant |-> "Destroy", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "destroy_mob", source_machine |-> "external_entry", source_effect |-> "Destroy", effect_id |-> 0])
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "mob", variant |-> "Destroy", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "destroy_mob", source_machine |-> "external_entry", source_effect |-> "Destroy", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "Destroy", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "destroy_mob", source_machine |-> "external_entry", source_effect |-> "Destroy", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

DeliverQueuedRoute ==
    /\ Len(pending_routes) > 0
    /\ LET route == Head(pending_routes) IN
       /\ pending_routes' = Tail(pending_routes)
       /\ delivered_routes' = delivered_routes \cup {route}
       /\ model_step_count' = model_step_count + 1
       /\ pending_inputs' = AppendIfMissing(pending_inputs, [machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id]}
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

RejectPendingEntryInput ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.source_kind = "entry"
       /\ ~EntryPacketAdmissible(packet)
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions
       /\ model_step_count' = model_step_count + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_current_run_id, meerkat_pre_run_phase, meerkat_silent_intent_overrides, meerkat_realtime_intent_present, meerkat_realtime_binding_state, meerkat_realtime_binding_authority_epoch, meerkat_realtime_reattach_required, meerkat_realtime_next_authority_epoch, meerkat_live_topology_phase, meerkat_mcp_server_states, mob_phase, mob_live_runtime_ids, mob_externally_addressable_runtime_ids, mob_runtime_fence_tokens, mob_active_run_count, mob_pending_spawn_count, mob_coordinator_bound, mob_member_state_markers, mob_wiring_edges, mob_identity_to_runtime, mob_tasks, mob_in_progress_task_ids, mob_completed_task_ids, witness_current_script_input, witness_remaining_script_inputs >>

QuiescentStutter ==
    /\ Len(pending_routes) = 0
    /\ Len(pending_inputs) = 0
    /\ UNCHANGED vars

WitnessInjectNext_basic_round_trip ==
    FALSE

WitnessInjectNext_retire_runtime_path ==
    FALSE

WitnessInjectNext_destroy_runtime_path ==
    FALSE

CoreNext ==
    \/ DeliverQueuedRoute
    \/ RejectPendingEntryInput
    \/ meerkat_Initialize
    \/ \E arg_session_id \in SessionIdValues : meerkat_RegisterSessionIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_RegisterSessionAttached(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_RegisterSessionRunning(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_RegisterSessionRetired(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_RegisterSessionStopped(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_UnregisterSessionIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_UnregisterSessionAttached(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_UnregisterSessionRunning(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_UnregisterSessionRetired(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_UnregisterSessionStopped(arg_session_id)
    \/ \E arg_previous_identity \in SessionLlmIdentityValues : \E arg_previous_visibility_state \in SessionToolVisibilityStateValues : \E arg_previous_capability_surface \in OptionSessionLlmCapabilitySurfaceValues : \E arg_previous_capability_surface_status \in SessionLlmCapabilitySurfaceStatusValues : \E arg_target_identity \in SessionLlmIdentityValues : \E arg_target_capability_surface \in SessionLlmCapabilitySurfaceValues : \E arg_next_visibility_state \in SessionToolVisibilityStateValues : \E arg_next_capability_base_filter \in ToolFilterValues : \E arg_next_active_visibility_revision \in 0..2 : \E arg_tool_visibility_delta \in SessionToolVisibilityDeltaValues : meerkat_ReconfigureSessionLlmIdentityAttached(arg_previous_identity, arg_previous_visibility_state, arg_previous_capability_surface, arg_previous_capability_surface_status, arg_target_identity, arg_target_capability_surface, arg_next_visibility_state, arg_next_capability_base_filter, arg_next_active_visibility_revision, arg_tool_visibility_delta)
    \/ \E arg_previous_identity \in SessionLlmIdentityValues : \E arg_previous_visibility_state \in SessionToolVisibilityStateValues : \E arg_previous_capability_surface \in OptionSessionLlmCapabilitySurfaceValues : \E arg_previous_capability_surface_status \in SessionLlmCapabilitySurfaceStatusValues : \E arg_target_identity \in SessionLlmIdentityValues : \E arg_target_capability_surface \in SessionLlmCapabilitySurfaceValues : \E arg_next_visibility_state \in SessionToolVisibilityStateValues : \E arg_next_capability_base_filter \in ToolFilterValues : \E arg_next_active_visibility_revision \in 0..2 : \E arg_tool_visibility_delta \in SessionToolVisibilityDeltaValues : meerkat_ReconfigureSessionLlmIdentityRunning(arg_previous_identity, arg_previous_visibility_state, arg_previous_capability_surface, arg_previous_capability_surface_status, arg_target_identity, arg_target_capability_surface, arg_next_visibility_state, arg_next_capability_base_filter, arg_next_active_visibility_revision, arg_tool_visibility_delta)
    \/ \E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterIdle(arg_filter, arg_witnesses)
    \/ \E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterAttached(arg_filter, arg_witnesses)
    \/ \E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterRunning(arg_filter, arg_witnesses)
    \/ \E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterRetired(arg_filter, arg_witnesses)
    \/ \E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterStopped(arg_filter, arg_witnesses)
    \/ \E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsIdle(arg_names, arg_witnesses)
    \/ \E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsAttached(arg_names, arg_witnesses)
    \/ \E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsRunning(arg_names, arg_witnesses)
    \/ \E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsRetired(arg_names, arg_witnesses)
    \/ \E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsStopped(arg_names, arg_witnesses)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsInitializing(arg_agent_runtime_id, arg_fence_token, arg_generation)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsIdle(arg_agent_runtime_id, arg_fence_token, arg_generation)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsAttached(arg_agent_runtime_id, arg_fence_token, arg_generation)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsRunning(arg_agent_runtime_id, arg_fence_token, arg_generation)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsRetired(arg_agent_runtime_id, arg_fence_token, arg_generation)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsStopped(arg_agent_runtime_id, arg_fence_token, arg_generation)
    \/ \E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextIdle(arg_keep_alive)
    \/ \E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextAttached(arg_keep_alive)
    \/ \E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextRunning(arg_keep_alive)
    \/ \E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextRetired(arg_keep_alive)
    \/ \E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextStopped(arg_keep_alive)
    \/ \E arg_reason \in StringValues : meerkat_NotifyDrainExitedIdle(arg_reason)
    \/ \E arg_reason \in StringValues : meerkat_NotifyDrainExitedAttached(arg_reason)
    \/ \E arg_reason \in StringValues : meerkat_NotifyDrainExitedRunning(arg_reason)
    \/ \E arg_reason \in StringValues : meerkat_NotifyDrainExitedRetired(arg_reason)
    \/ \E arg_reason \in StringValues : meerkat_NotifyDrainExitedStopped(arg_reason)
    \/ meerkat_InterruptCurrentRunAttached
    \/ meerkat_InterruptCurrentRun
    \/ meerkat_CancelAfterBoundaryAttached
    \/ meerkat_CancelAfterBoundary
    \/ \E arg_revision \in 0..2 : meerkat_BoundaryAppliedPublish(arg_revision)
    \/ \E arg_active_filter \in ToolFilterValues : \E arg_staged_filter \in ToolFilterValues : \E arg_active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : meerkat_PublishCommittedVisibleSetIdle(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)
    \/ \E arg_active_filter \in ToolFilterValues : \E arg_staged_filter \in ToolFilterValues : \E arg_active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : meerkat_PublishCommittedVisibleSetAttached(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)
    \/ \E arg_active_filter \in ToolFilterValues : \E arg_staged_filter \in ToolFilterValues : \E arg_active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : meerkat_PublishCommittedVisibleSetRunning(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)
    \/ \E arg_active_filter \in ToolFilterValues : \E arg_staged_filter \in ToolFilterValues : \E arg_active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : meerkat_PublishCommittedVisibleSetRetired(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)
    \/ \E arg_active_filter \in ToolFilterValues : \E arg_staged_filter \in ToolFilterValues : \E arg_active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : meerkat_PublishCommittedVisibleSetStopped(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)
    \/ meerkat_RetireRequestedFromIdle
    \/ meerkat_Reset
    \/ meerkat_StopRuntimeExecutorUnbound
    \/ meerkat_StopRuntimeExecutorAttached
    \/ meerkat_StopRuntimeExecutorRunning
    \/ meerkat_Destroy
    \/ \E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorAttached(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorRunning(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorRetired(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorStopped(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsIdle(arg_session_id, arg_intents)
    \/ \E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsAttached(arg_session_id, arg_intents)
    \/ \E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsRunning(arg_session_id, arg_intents)
    \/ \E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsRetired(arg_session_id, arg_intents)
    \/ \E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsStopped(arg_session_id, arg_intents)
    \/ \E arg_session_id \in SessionIdValues : meerkat_AbortIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_AbortAttached(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_AbortRunning(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_AbortRetired(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_AbortStopped(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_WaitIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_WaitAttached(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_WaitRunning(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_WaitRetired(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_WaitStopped(arg_session_id)
    \/ meerkat_AbortAllIdle
    \/ meerkat_AbortAllAttached
    \/ meerkat_AbortAllRunning
    \/ meerkat_AbortAllRetired
    \/ meerkat_AbortAllStopped
    \/ meerkat_EnsureDrainRunningAttached
    \/ meerkat_EnsureDrainRunningRunning
    \/ \E arg_runtime_id \in AgentRuntimeIdValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : meerkat_IngestIdle(arg_runtime_id, arg_work_id, arg_origin)
    \/ \E arg_runtime_id \in AgentRuntimeIdValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : meerkat_IngestAttached(arg_runtime_id, arg_work_id, arg_origin)
    \/ \E arg_runtime_id \in AgentRuntimeIdValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : meerkat_IngestRunning(arg_runtime_id, arg_work_id, arg_origin)
    \/ \E arg_kind \in StringValues : meerkat_PublishEventIdle(arg_kind)
    \/ \E arg_kind \in StringValues : meerkat_PublishEventAttached(arg_kind)
    \/ \E arg_kind \in StringValues : meerkat_PublishEventRunning(arg_kind)
    \/ \E arg_kind \in StringValues : meerkat_PublishEventRetired(arg_kind)
    \/ \E arg_kind \in StringValues : meerkat_PublishEventStopped(arg_kind)
    \/ \E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionIdleQueued(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)
    \/ \E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionIdleImmediate(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)
    \/ \E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionAttachedImmediate(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)
    \/ \E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionAttachedQueued(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)
    \/ \E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionRunningQueuedPassive(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)
    \/ \E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionRunningQueuedWakeIfIdle(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)
    \/ \E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionRunningInterruptYielding(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)
    \/ \E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionRunningImmediate(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)
    \/ \E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeIdle(arg_input_id)
    \/ \E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeAttached(arg_input_id)
    \/ \E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeRunning(arg_input_id)
    \/ meerkat_ClassifyExternalEnvelopeAttached
    \/ meerkat_ClassifyExternalEnvelopeRunning
    \/ meerkat_ClassifyPlainEventAttached
    \/ meerkat_ClassifyPlainEventRunning
    \/ \E arg_session_id \in SessionIdValues : \E arg_run_id \in RunIdValues : meerkat_PrepareIdle(arg_session_id, arg_run_id)
    \/ \E arg_session_id \in SessionIdValues : \E arg_run_id \in RunIdValues : meerkat_PrepareAttached(arg_session_id, arg_run_id)
    \/ \E arg_run_id \in RunIdValues : meerkat_DrainQueuedRunRetired(arg_run_id)
    \/ meerkat_StartConversationRunAttached
    \/ meerkat_StartImmediateAppendAttached
    \/ meerkat_StartImmediateContextAttached
    \/ \E arg_input_id \in InputIdValues : \E arg_run_id \in RunIdValues : meerkat_CommitRunningToIdle(arg_input_id, arg_run_id)
    \/ \E arg_input_id \in InputIdValues : \E arg_run_id \in RunIdValues : meerkat_CommitRunningToAttached(arg_input_id, arg_run_id)
    \/ \E arg_input_id \in InputIdValues : \E arg_run_id \in RunIdValues : meerkat_CommitRunningToRetired(arg_input_id, arg_run_id)
    \/ \E arg_run_id \in RunIdValues : meerkat_FailRunningToIdle(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : meerkat_FailRunningToAttached(arg_run_id)
    \/ \E arg_run_id \in RunIdValues : meerkat_FailRunningToRetired(arg_run_id)
    \/ meerkat_StageAddAttached
    \/ meerkat_StageAddRunning
    \/ meerkat_StageRemoveAttached
    \/ meerkat_StageRemoveRunning
    \/ meerkat_StageReloadAttached
    \/ meerkat_StageReloadRunning
    \/ meerkat_ApplySurfaceBoundaryAttached
    \/ meerkat_ApplySurfaceBoundaryRunning
    \/ meerkat_PendingSucceededAttached
    \/ meerkat_PendingSucceededRunning
    \/ meerkat_PendingFailedAttached
    \/ meerkat_PendingFailedRunning
    \/ meerkat_CallStartedAttached
    \/ meerkat_CallStartedRunning
    \/ meerkat_CallFinishedAttached
    \/ meerkat_CallFinishedRunning
    \/ meerkat_FinalizeRemovalCleanAttached
    \/ meerkat_FinalizeRemovalCleanRunning
    \/ meerkat_FinalizeRemovalForcedAttached
    \/ meerkat_FinalizeRemovalForcedRunning
    \/ meerkat_SnapshotAlignedAttached
    \/ meerkat_SnapshotAlignedRunning
    \/ meerkat_ShutdownSurfaceAttached
    \/ meerkat_ShutdownSurfaceRunning
    \/ meerkat_RecycleFromIdleOrRetired
    \/ meerkat_RecycleFromAttached
    \/ \E arg_present \in BOOLEAN : meerkat_ProjectRealtimeIntentIdle(arg_present)
    \/ \E arg_present \in BOOLEAN : meerkat_ProjectRealtimeIntentAttached(arg_present)
    \/ \E arg_present \in BOOLEAN : meerkat_ProjectRealtimeIntentRunning(arg_present)
    \/ \E arg_present \in BOOLEAN : meerkat_ProjectRealtimeIntentRetired(arg_present)
    \/ \E arg_present \in BOOLEAN : meerkat_ProjectRealtimeIntentStopped(arg_present)
    \/ meerkat_BeginRealtimeBindingIdle
    \/ meerkat_BeginRealtimeBindingAttached
    \/ meerkat_BeginRealtimeBindingRunning
    \/ meerkat_BeginRealtimeBindingRetired
    \/ meerkat_BeginRealtimeBindingStopped
    \/ meerkat_ReplaceRealtimeBindingIdle
    \/ meerkat_ReplaceRealtimeBindingAttached
    \/ meerkat_ReplaceRealtimeBindingRunning
    \/ meerkat_ReplaceRealtimeBindingRetired
    \/ meerkat_ReplaceRealtimeBindingStopped
    \/ meerkat_DetachRealtimeBindingIdle
    \/ meerkat_DetachRealtimeBindingAttached
    \/ meerkat_DetachRealtimeBindingRunning
    \/ meerkat_DetachRealtimeBindingRetired
    \/ meerkat_DetachRealtimeBindingStopped
    \/ meerkat_RequireRealtimeReattachIdle
    \/ meerkat_RequireRealtimeReattachAttached
    \/ meerkat_RequireRealtimeReattachRunning
    \/ meerkat_RequireRealtimeReattachRetired
    \/ meerkat_RequireRealtimeReattachStopped
    \/ \E arg_authority_epoch \in 0..2 : \E arg_next_binding_state \in RealtimeBindingStateValues : meerkat_PublishRealtimeSignalIdle(arg_authority_epoch, arg_next_binding_state)
    \/ \E arg_authority_epoch \in 0..2 : \E arg_next_binding_state \in RealtimeBindingStateValues : meerkat_PublishRealtimeSignalAttached(arg_authority_epoch, arg_next_binding_state)
    \/ \E arg_authority_epoch \in 0..2 : \E arg_next_binding_state \in RealtimeBindingStateValues : meerkat_PublishRealtimeSignalRunning(arg_authority_epoch, arg_next_binding_state)
    \/ \E arg_authority_epoch \in 0..2 : \E arg_next_binding_state \in RealtimeBindingStateValues : meerkat_PublishRealtimeSignalRetired(arg_authority_epoch, arg_next_binding_state)
    \/ \E arg_authority_epoch \in 0..2 : \E arg_next_binding_state \in RealtimeBindingStateValues : meerkat_PublishRealtimeSignalStopped(arg_authority_epoch, arg_next_binding_state)
    \/ \E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectPendingIdle(arg_server_id)
    \/ \E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectPendingAttached(arg_server_id)
    \/ \E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectPendingRunning(arg_server_id)
    \/ \E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectPendingRetired(arg_server_id)
    \/ \E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectPendingStopped(arg_server_id)
    \/ \E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectedIdle(arg_server_id)
    \/ \E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectedAttached(arg_server_id)
    \/ \E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectedRunning(arg_server_id)
    \/ \E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectedRetired(arg_server_id)
    \/ \E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectedStopped(arg_server_id)
    \/ \E arg_server_id \in McpServerIdValues : \E arg_error \in StringValues : meerkat_McpServerFailedIdle(arg_server_id, arg_error)
    \/ \E arg_server_id \in McpServerIdValues : \E arg_error \in StringValues : meerkat_McpServerFailedAttached(arg_server_id, arg_error)
    \/ \E arg_server_id \in McpServerIdValues : \E arg_error \in StringValues : meerkat_McpServerFailedRunning(arg_server_id, arg_error)
    \/ \E arg_server_id \in McpServerIdValues : \E arg_error \in StringValues : meerkat_McpServerFailedRetired(arg_server_id, arg_error)
    \/ \E arg_server_id \in McpServerIdValues : \E arg_error \in StringValues : meerkat_McpServerFailedStopped(arg_server_id, arg_error)
    \/ \E arg_server_id \in McpServerIdValues : meerkat_McpServerDisconnectedIdle(arg_server_id)
    \/ \E arg_server_id \in McpServerIdValues : meerkat_McpServerDisconnectedAttached(arg_server_id)
    \/ \E arg_server_id \in McpServerIdValues : meerkat_McpServerDisconnectedRunning(arg_server_id)
    \/ \E arg_server_id \in McpServerIdValues : meerkat_McpServerDisconnectedRetired(arg_server_id)
    \/ \E arg_server_id \in McpServerIdValues : meerkat_McpServerDisconnectedStopped(arg_server_id)
    \/ \E arg_server_id \in McpServerIdValues : meerkat_McpServerReloadIdle(arg_server_id)
    \/ \E arg_server_id \in McpServerIdValues : meerkat_McpServerReloadAttached(arg_server_id)
    \/ \E arg_server_id \in McpServerIdValues : meerkat_McpServerReloadRunning(arg_server_id)
    \/ \E arg_server_id \in McpServerIdValues : meerkat_McpServerReloadRetired(arg_server_id)
    \/ \E arg_server_id \in McpServerIdValues : meerkat_McpServerReloadStopped(arg_server_id)
    \/ \E arg_authority_epoch \in 0..2 : meerkat_BeginLiveTopologyReconfigureIdle(arg_authority_epoch)
    \/ \E arg_authority_epoch \in 0..2 : meerkat_BeginLiveTopologyReconfigureAttached(arg_authority_epoch)
    \/ \E arg_authority_epoch \in 0..2 : meerkat_BeginLiveTopologyReconfigureRunning(arg_authority_epoch)
    \/ \E arg_authority_epoch \in 0..2 : meerkat_BeginLiveTopologyReconfigureRetired(arg_authority_epoch)
    \/ \E arg_authority_epoch \in 0..2 : meerkat_BeginLiveTopologyReconfigureStopped(arg_authority_epoch)
    \/ meerkat_MarkLiveTopologyDetachedIdle
    \/ meerkat_MarkLiveTopologyDetachedAttached
    \/ meerkat_MarkLiveTopologyDetachedRunning
    \/ meerkat_MarkLiveTopologyDetachedRetired
    \/ meerkat_MarkLiveTopologyDetachedStopped
    \/ meerkat_ApplyLiveTopologyIdentityIdle
    \/ meerkat_ApplyLiveTopologyIdentityAttached
    \/ meerkat_ApplyLiveTopologyIdentityRunning
    \/ meerkat_ApplyLiveTopologyIdentityRetired
    \/ meerkat_ApplyLiveTopologyIdentityStopped
    \/ meerkat_ApplyLiveTopologyVisibilityIdle
    \/ meerkat_ApplyLiveTopologyVisibilityAttached
    \/ meerkat_ApplyLiveTopologyVisibilityRunning
    \/ meerkat_ApplyLiveTopologyVisibilityRetired
    \/ meerkat_ApplyLiveTopologyVisibilityStopped
    \/ meerkat_CompleteLiveTopologyIdle
    \/ meerkat_CompleteLiveTopologyAttached
    \/ meerkat_CompleteLiveTopologyRunning
    \/ meerkat_CompleteLiveTopologyRetired
    \/ meerkat_CompleteLiveTopologyStopped
    \/ meerkat_AbortLiveTopologyBeforeDetachIdle
    \/ meerkat_AbortLiveTopologyBeforeDetachAttached
    \/ meerkat_AbortLiveTopologyBeforeDetachRunning
    \/ meerkat_AbortLiveTopologyBeforeDetachRetired
    \/ meerkat_AbortLiveTopologyBeforeDetachStopped
    \/ meerkat_FailLiveTopologyAfterDetachIdle
    \/ meerkat_FailLiveTopologyAfterDetachAttached
    \/ meerkat_FailLiveTopologyAfterDetachRunning
    \/ meerkat_FailLiveTopologyAfterDetachRetired
    \/ meerkat_FailLiveTopologyAfterDetachStopped
    \/ \E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : \E arg_external_addressable \in BOOLEAN : mob_SpawnRunning(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation, arg_external_addressable)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeReady(arg_agent_runtime_id, arg_fence_token)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : mob_SubmitWorkRunningExternal(arg_agent_runtime_id, arg_fence_token, arg_work_id, arg_origin)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : mob_SubmitWorkRunningInternal(arg_agent_runtime_id, arg_fence_token, arg_work_id, arg_origin)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_RetireMember(arg_agent_runtime_id, arg_fence_token)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeRetired(arg_agent_runtime_id, arg_fence_token)
    \/ \E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : \E arg_external_addressable \in BOOLEAN : mob_ResetMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation, arg_external_addressable)
    \/ \E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : \E arg_external_addressable \in BOOLEAN : mob_RespawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation, arg_external_addressable)
    \/ mob_MarkCompleted
    \/ mob_DestroyMob
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeDestroyed(arg_agent_runtime_id, arg_fence_token)
    \/ mob_RecordOperatorActionProvenanceRunning
    \/ mob_RecordOperatorActionProvenanceStopped
    \/ mob_RecordOperatorActionProvenanceCompleted
    \/ mob_RecordOperatorActionProvenanceDestroyed
    \/ mob_SetSpawnPolicyRunning
    \/ mob_SetSpawnPolicyStopped
    \/ mob_SetSpawnPolicyCompleted
    \/ mob_SetSpawnPolicyDestroyed
    \/ mob_StopRunning
    \/ mob_ResumeStopped
    \/ mob_CompleteRunning
    \/ mob_ResetToRunning
    \/ mob_WireRunning
    \/ mob_ExternalTurnRunning
    \/ mob_InternalTurnRunning
    \/ \E arg_task_id \in TaskIdValues : \E arg_task_payload \in MobTaskValues : mob_TaskCreateRunning(arg_task_id, arg_task_payload)
    \/ \E arg_task_id \in TaskIdValues : \E arg_new_status \in TaskStatusValues : mob_TaskUpdateRunningPending(arg_task_id, arg_new_status)
    \/ \E arg_task_id \in TaskIdValues : \E arg_new_status \in TaskStatusValues : mob_TaskUpdateRunningInProgress(arg_task_id, arg_new_status)
    \/ \E arg_task_id \in TaskIdValues : \E arg_new_status \in TaskStatusValues : mob_TaskUpdateRunningCompleted(arg_task_id, arg_new_status)
    \/ \E arg_task_id \in TaskIdValues : \E arg_new_status \in TaskStatusValues : mob_TaskUpdateRunningCancelled(arg_task_id, arg_new_status)
    \/ mob_ForceCancelRunning
    \/ mob_SubscribeAgentEventsRunning
    \/ mob_SubscribeAgentEventsStopped
    \/ mob_SubscribeAgentEventsCompleted
    \/ mob_SubscribeAgentEventsDestroyed
    \/ mob_SubscribeAllAgentEventsRunning
    \/ mob_SubscribeAllAgentEventsStopped
    \/ mob_SubscribeAllAgentEventsCompleted
    \/ mob_SubscribeAllAgentEventsDestroyed
    \/ mob_SubscribeMobEventsRunning
    \/ mob_SubscribeMobEventsStopped
    \/ mob_SubscribeMobEventsCompleted
    \/ mob_SubscribeMobEventsDestroyed
    \/ mob_ShutdownRunning
    \/ mob_ShutdownStopped
    \/ mob_ShutdownCompleted
    \/ mob_CancelFlowRunning
    \/ mob_InitializeOrchestratorRunning
    \/ mob_BindCoordinatorRunning
    \/ mob_UnbindCoordinatorRunning
    \/ mob_StageSpawnRunning
    \/ mob_StopOrchestratorRunning
    \/ mob_StopOrchestratorStopped
    \/ mob_StopOrchestratorCompleted
    \/ mob_ResumeOrchestratorRunning
    \/ mob_ResumeOrchestratorStopped
    \/ mob_ResumeOrchestratorCompleted
    \/ mob_DestroyOrchestratorRunning
    \/ mob_DestroyOrchestratorStopped
    \/ mob_DestroyOrchestratorCompleted
    \/ mob_ForceCancelMemberRunning
    \/ mob_MemberPeerExposedRunning
    \/ mob_MemberTerminalizedRunning
    \/ mob_OperationPeerTrustedRunning
    \/ mob_PeerInputAdmittedRunning
    \/ mob_BeginCleanupStopped
    \/ mob_BeginCleanupCompleted
    \/ mob_FinishCleanupStopped
    \/ mob_FinishCleanupCompleted
    \/ mob_RunFlowRunning
    \/ mob_StartFlowRunning
    \/ mob_CreateRunRunning
    \/ mob_StartRunRunning
    \/ mob_UnwireRunning
    \/ mob_CompleteFlowRunning
    \/ mob_FinishRunRunning
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireRunning(arg_agent_runtime_id)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireStopped(arg_agent_runtime_id)
    \/ mob_RetireAllRunning
    \/ mob_RetireAllStopped
    \/ mob_CompleteSpawnRunning
    \/ mob_DestroyFromAny
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RespawnRunning(arg_agent_runtime_id)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_CancelAllWorkRunning(arg_agent_runtime_id, arg_fence_token)
    \/ QuiescentStutter

InjectNext ==
    \/ \E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : \E arg_external_addressable \in BOOLEAN : Inject_spawn_member(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation, arg_external_addressable)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : Inject_submit_work(arg_agent_runtime_id, arg_fence_token, arg_work_id, arg_origin)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : Inject_retire_member(arg_agent_runtime_id)
    \/ Inject_destroy_mob

Next ==
    \/ CoreNext
    \/ InjectNext

WitnessNext_basic_round_trip ==
    \/ CoreNext
    \/ WitnessInjectNext_basic_round_trip

WitnessNext_retire_runtime_path ==
    \/ CoreNext
    \/ WitnessInjectNext_retire_runtime_path

WitnessNext_destroy_runtime_path ==
    \/ CoreNext
    \/ WitnessInjectNext_destroy_runtime_path


RouteObserved_binding_request_reaches_meerkat == \E packet \in RoutePackets : packet.route = "binding_request_reaches_meerkat"
RouteCoverage_binding_request_reaches_meerkat == (RouteObserved_binding_request_reaches_meerkat \/ ~RouteObserved_binding_request_reaches_meerkat)
RouteObserved_work_request_reaches_meerkat == \E packet \in RoutePackets : packet.route = "work_request_reaches_meerkat"
RouteCoverage_work_request_reaches_meerkat == (RouteObserved_work_request_reaches_meerkat \/ ~RouteObserved_work_request_reaches_meerkat)
RouteObserved_retire_request_reaches_meerkat == \E packet \in RoutePackets : packet.route = "retire_request_reaches_meerkat"
RouteCoverage_retire_request_reaches_meerkat == (RouteObserved_retire_request_reaches_meerkat \/ ~RouteObserved_retire_request_reaches_meerkat)
RouteObserved_destroy_request_reaches_meerkat == \E packet \in RoutePackets : packet.route = "destroy_request_reaches_meerkat"
RouteCoverage_destroy_request_reaches_meerkat == (RouteObserved_destroy_request_reaches_meerkat \/ ~RouteObserved_destroy_request_reaches_meerkat)
RouteObserved_runtime_bound_reaches_mob == \E packet \in RoutePackets : packet.route = "runtime_bound_reaches_mob"
RouteCoverage_runtime_bound_reaches_mob == (RouteObserved_runtime_bound_reaches_mob \/ ~RouteObserved_runtime_bound_reaches_mob)
RouteObserved_runtime_retired_reaches_mob == \E packet \in RoutePackets : packet.route = "runtime_retired_reaches_mob"
RouteCoverage_runtime_retired_reaches_mob == (RouteObserved_runtime_retired_reaches_mob \/ ~RouteObserved_runtime_retired_reaches_mob)
RouteObserved_runtime_destroyed_reaches_mob == \E packet \in RoutePackets : packet.route = "runtime_destroyed_reaches_mob"
RouteCoverage_runtime_destroyed_reaches_mob == (RouteObserved_runtime_destroyed_reaches_mob \/ ~RouteObserved_runtime_destroyed_reaches_mob)
CoverageInstrumentation == RouteCoverage_binding_request_reaches_meerkat /\ RouteCoverage_work_request_reaches_meerkat /\ RouteCoverage_retire_request_reaches_meerkat /\ RouteCoverage_destroy_request_reaches_meerkat /\ RouteCoverage_runtime_bound_reaches_mob /\ RouteCoverage_runtime_retired_reaches_mob /\ RouteCoverage_runtime_destroyed_reaches_mob

CiStateConstraint == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(meerkat_silent_intent_overrides) <= 0 /\ Cardinality(DOMAIN meerkat_mcp_server_states) <= 0 /\ Cardinality(mob_live_runtime_ids) <= 0 /\ Cardinality(mob_externally_addressable_runtime_ids) <= 0 /\ Cardinality(DOMAIN mob_runtime_fence_tokens) <= 0 /\ Cardinality(DOMAIN mob_member_state_markers) <= 0 /\ Cardinality(mob_wiring_edges) <= 0 /\ Cardinality(DOMAIN mob_identity_to_runtime) <= 0 /\ Cardinality(DOMAIN mob_tasks) <= 0 /\ Cardinality(mob_in_progress_task_ids) <= 0 /\ Cardinality(mob_completed_task_ids) <= 0
DeepStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 2 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(meerkat_silent_intent_overrides) <= 2 /\ Cardinality(DOMAIN meerkat_mcp_server_states) <= 2 /\ Cardinality(mob_live_runtime_ids) <= 2 /\ Cardinality(mob_externally_addressable_runtime_ids) <= 2 /\ Cardinality(DOMAIN mob_runtime_fence_tokens) <= 2 /\ Cardinality(DOMAIN mob_member_state_markers) <= 2 /\ Cardinality(mob_wiring_edges) <= 2 /\ Cardinality(DOMAIN mob_identity_to_runtime) <= 2 /\ Cardinality(DOMAIN mob_tasks) <= 2 /\ Cardinality(mob_in_progress_task_ids) <= 2 /\ Cardinality(mob_completed_task_ids) <= 2
WitnessStateConstraint_basic_round_trip == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 13 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 3 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(meerkat_silent_intent_overrides) <= 0 /\ Cardinality(DOMAIN meerkat_mcp_server_states) <= 0 /\ Cardinality(mob_live_runtime_ids) <= 0 /\ Cardinality(mob_externally_addressable_runtime_ids) <= 0 /\ Cardinality(DOMAIN mob_runtime_fence_tokens) <= 0 /\ Cardinality(DOMAIN mob_member_state_markers) <= 0 /\ Cardinality(mob_wiring_edges) <= 0 /\ Cardinality(DOMAIN mob_identity_to_runtime) <= 0 /\ Cardinality(DOMAIN mob_tasks) <= 0 /\ Cardinality(mob_in_progress_task_ids) <= 0 /\ Cardinality(mob_completed_task_ids) <= 0
WitnessStateConstraint_retire_runtime_path == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 12 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(meerkat_silent_intent_overrides) <= 0 /\ Cardinality(DOMAIN meerkat_mcp_server_states) <= 0 /\ Cardinality(mob_live_runtime_ids) <= 0 /\ Cardinality(mob_externally_addressable_runtime_ids) <= 0 /\ Cardinality(DOMAIN mob_runtime_fence_tokens) <= 0 /\ Cardinality(DOMAIN mob_member_state_markers) <= 0 /\ Cardinality(mob_wiring_edges) <= 0 /\ Cardinality(DOMAIN mob_identity_to_runtime) <= 0 /\ Cardinality(DOMAIN mob_tasks) <= 0 /\ Cardinality(mob_in_progress_task_ids) <= 0 /\ Cardinality(mob_completed_task_ids) <= 0
WitnessStateConstraint_destroy_runtime_path == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 12 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(meerkat_silent_intent_overrides) <= 0 /\ Cardinality(DOMAIN meerkat_mcp_server_states) <= 0 /\ Cardinality(mob_live_runtime_ids) <= 0 /\ Cardinality(mob_externally_addressable_runtime_ids) <= 0 /\ Cardinality(DOMAIN mob_runtime_fence_tokens) <= 0 /\ Cardinality(DOMAIN mob_member_state_markers) <= 0 /\ Cardinality(mob_wiring_edges) <= 0 /\ Cardinality(DOMAIN mob_identity_to_runtime) <= 0 /\ Cardinality(DOMAIN mob_tasks) <= 0 /\ Cardinality(mob_in_progress_task_ids) <= 0 /\ Cardinality(mob_completed_task_ids) <= 0

Spec == Init /\ [][Next]_vars
WitnessSpec_basic_round_trip == WitnessInit_basic_round_trip /\ [] [WitnessNext_basic_round_trip]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(RejectPendingEntryInput) /\ WF_vars(meerkat_Initialize) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSessionIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSessionAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSessionRunning(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSessionRetired(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSessionStopped(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSessionIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSessionAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSessionRunning(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSessionRetired(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSessionStopped(arg_session_id)) /\ WF_vars(\E arg_previous_identity \in SessionLlmIdentityValues : \E arg_previous_visibility_state \in SessionToolVisibilityStateValues : \E arg_previous_capability_surface \in OptionSessionLlmCapabilitySurfaceValues : \E arg_previous_capability_surface_status \in SessionLlmCapabilitySurfaceStatusValues : \E arg_target_identity \in SessionLlmIdentityValues : \E arg_target_capability_surface \in SessionLlmCapabilitySurfaceValues : \E arg_next_visibility_state \in SessionToolVisibilityStateValues : \E arg_next_capability_base_filter \in ToolFilterValues : \E arg_next_active_visibility_revision \in 0..2 : \E arg_tool_visibility_delta \in SessionToolVisibilityDeltaValues : meerkat_ReconfigureSessionLlmIdentityAttached(arg_previous_identity, arg_previous_visibility_state, arg_previous_capability_surface, arg_previous_capability_surface_status, arg_target_identity, arg_target_capability_surface, arg_next_visibility_state, arg_next_capability_base_filter, arg_next_active_visibility_revision, arg_tool_visibility_delta)) /\ WF_vars(\E arg_previous_identity \in SessionLlmIdentityValues : \E arg_previous_visibility_state \in SessionToolVisibilityStateValues : \E arg_previous_capability_surface \in OptionSessionLlmCapabilitySurfaceValues : \E arg_previous_capability_surface_status \in SessionLlmCapabilitySurfaceStatusValues : \E arg_target_identity \in SessionLlmIdentityValues : \E arg_target_capability_surface \in SessionLlmCapabilitySurfaceValues : \E arg_next_visibility_state \in SessionToolVisibilityStateValues : \E arg_next_capability_base_filter \in ToolFilterValues : \E arg_next_active_visibility_revision \in 0..2 : \E arg_tool_visibility_delta \in SessionToolVisibilityDeltaValues : meerkat_ReconfigureSessionLlmIdentityRunning(arg_previous_identity, arg_previous_visibility_state, arg_previous_capability_surface, arg_previous_capability_surface_status, arg_target_identity, arg_target_capability_surface, arg_next_visibility_state, arg_next_capability_base_filter, arg_next_active_visibility_revision, arg_tool_visibility_delta)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterIdle(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterAttached(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterRunning(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterRetired(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterStopped(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsIdle(arg_names, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsAttached(arg_names, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsRunning(arg_names, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsRetired(arg_names, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsStopped(arg_names, arg_witnesses)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsInitializing(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsIdle(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsAttached(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsRunning(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsRetired(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsStopped(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextIdle(arg_keep_alive)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextAttached(arg_keep_alive)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextRunning(arg_keep_alive)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextRetired(arg_keep_alive)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextStopped(arg_keep_alive)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedIdle(arg_reason)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedAttached(arg_reason)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedRunning(arg_reason)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedRetired(arg_reason)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedStopped(arg_reason)) /\ WF_vars(meerkat_InterruptCurrentRunAttached) /\ WF_vars(meerkat_InterruptCurrentRun) /\ WF_vars(meerkat_CancelAfterBoundaryAttached) /\ WF_vars(meerkat_CancelAfterBoundary) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_BoundaryAppliedPublish(arg_revision)) /\ WF_vars(\E arg_active_filter \in ToolFilterValues : \E arg_staged_filter \in ToolFilterValues : \E arg_active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : meerkat_PublishCommittedVisibleSetIdle(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)) /\ WF_vars(\E arg_active_filter \in ToolFilterValues : \E arg_staged_filter \in ToolFilterValues : \E arg_active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : meerkat_PublishCommittedVisibleSetAttached(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)) /\ WF_vars(\E arg_active_filter \in ToolFilterValues : \E arg_staged_filter \in ToolFilterValues : \E arg_active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : meerkat_PublishCommittedVisibleSetRunning(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)) /\ WF_vars(\E arg_active_filter \in ToolFilterValues : \E arg_staged_filter \in ToolFilterValues : \E arg_active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : meerkat_PublishCommittedVisibleSetRetired(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)) /\ WF_vars(\E arg_active_filter \in ToolFilterValues : \E arg_staged_filter \in ToolFilterValues : \E arg_active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : meerkat_PublishCommittedVisibleSetStopped(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)) /\ WF_vars(meerkat_RetireRequestedFromIdle) /\ WF_vars(meerkat_Reset) /\ WF_vars(meerkat_StopRuntimeExecutorUnbound) /\ WF_vars(meerkat_StopRuntimeExecutorAttached) /\ WF_vars(meerkat_StopRuntimeExecutorRunning) /\ WF_vars(meerkat_Destroy) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorRunning(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorRetired(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorStopped(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsIdle(arg_session_id, arg_intents)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsAttached(arg_session_id, arg_intents)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsRunning(arg_session_id, arg_intents)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsRetired(arg_session_id, arg_intents)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsStopped(arg_session_id, arg_intents)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortRunning(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortRetired(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortStopped(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitRunning(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitRetired(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitStopped(arg_session_id)) /\ WF_vars(meerkat_AbortAllIdle) /\ WF_vars(meerkat_AbortAllAttached) /\ WF_vars(meerkat_AbortAllRunning) /\ WF_vars(meerkat_AbortAllRetired) /\ WF_vars(meerkat_AbortAllStopped) /\ WF_vars(meerkat_EnsureDrainRunningAttached) /\ WF_vars(meerkat_EnsureDrainRunningRunning) /\ WF_vars(\E arg_runtime_id \in AgentRuntimeIdValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : meerkat_IngestIdle(arg_runtime_id, arg_work_id, arg_origin)) /\ WF_vars(\E arg_runtime_id \in AgentRuntimeIdValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : meerkat_IngestAttached(arg_runtime_id, arg_work_id, arg_origin)) /\ WF_vars(\E arg_runtime_id \in AgentRuntimeIdValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : meerkat_IngestRunning(arg_runtime_id, arg_work_id, arg_origin)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventIdle(arg_kind)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventAttached(arg_kind)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventRunning(arg_kind)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventRetired(arg_kind)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventStopped(arg_kind)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionIdleQueued(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionIdleImmediate(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionAttachedImmediate(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionAttachedQueued(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionRunningQueuedPassive(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionRunningQueuedWakeIfIdle(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionRunningInterruptYielding(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionRunningImmediate(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeIdle(arg_input_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeAttached(arg_input_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeRunning(arg_input_id)) /\ WF_vars(meerkat_ClassifyExternalEnvelopeAttached) /\ WF_vars(meerkat_ClassifyExternalEnvelopeRunning) /\ WF_vars(meerkat_ClassifyPlainEventAttached) /\ WF_vars(meerkat_ClassifyPlainEventRunning) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_run_id \in RunIdValues : meerkat_PrepareIdle(arg_session_id, arg_run_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_run_id \in RunIdValues : meerkat_PrepareAttached(arg_session_id, arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : meerkat_DrainQueuedRunRetired(arg_run_id)) /\ WF_vars(meerkat_StartConversationRunAttached) /\ WF_vars(meerkat_StartImmediateAppendAttached) /\ WF_vars(meerkat_StartImmediateContextAttached) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_run_id \in RunIdValues : meerkat_CommitRunningToIdle(arg_input_id, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_run_id \in RunIdValues : meerkat_CommitRunningToAttached(arg_input_id, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_run_id \in RunIdValues : meerkat_CommitRunningToRetired(arg_input_id, arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : meerkat_FailRunningToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : meerkat_FailRunningToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : meerkat_FailRunningToRetired(arg_run_id)) /\ WF_vars(meerkat_StageAddAttached) /\ WF_vars(meerkat_StageAddRunning) /\ WF_vars(meerkat_StageRemoveAttached) /\ WF_vars(meerkat_StageRemoveRunning) /\ WF_vars(meerkat_StageReloadAttached) /\ WF_vars(meerkat_StageReloadRunning) /\ WF_vars(meerkat_ApplySurfaceBoundaryAttached) /\ WF_vars(meerkat_ApplySurfaceBoundaryRunning) /\ WF_vars(meerkat_PendingSucceededAttached) /\ WF_vars(meerkat_PendingSucceededRunning) /\ WF_vars(meerkat_PendingFailedAttached) /\ WF_vars(meerkat_PendingFailedRunning) /\ WF_vars(meerkat_CallStartedAttached) /\ WF_vars(meerkat_CallStartedRunning) /\ WF_vars(meerkat_CallFinishedAttached) /\ WF_vars(meerkat_CallFinishedRunning) /\ WF_vars(meerkat_FinalizeRemovalCleanAttached) /\ WF_vars(meerkat_FinalizeRemovalCleanRunning) /\ WF_vars(meerkat_FinalizeRemovalForcedAttached) /\ WF_vars(meerkat_FinalizeRemovalForcedRunning) /\ WF_vars(meerkat_SnapshotAlignedAttached) /\ WF_vars(meerkat_SnapshotAlignedRunning) /\ WF_vars(meerkat_ShutdownSurfaceAttached) /\ WF_vars(meerkat_ShutdownSurfaceRunning) /\ WF_vars(meerkat_RecycleFromIdleOrRetired) /\ WF_vars(meerkat_RecycleFromAttached) /\ WF_vars(\E arg_present \in BOOLEAN : meerkat_ProjectRealtimeIntentIdle(arg_present)) /\ WF_vars(\E arg_present \in BOOLEAN : meerkat_ProjectRealtimeIntentAttached(arg_present)) /\ WF_vars(\E arg_present \in BOOLEAN : meerkat_ProjectRealtimeIntentRunning(arg_present)) /\ WF_vars(\E arg_present \in BOOLEAN : meerkat_ProjectRealtimeIntentRetired(arg_present)) /\ WF_vars(\E arg_present \in BOOLEAN : meerkat_ProjectRealtimeIntentStopped(arg_present)) /\ WF_vars(meerkat_BeginRealtimeBindingIdle) /\ WF_vars(meerkat_BeginRealtimeBindingAttached) /\ WF_vars(meerkat_BeginRealtimeBindingRunning) /\ WF_vars(meerkat_BeginRealtimeBindingRetired) /\ WF_vars(meerkat_BeginRealtimeBindingStopped) /\ WF_vars(meerkat_ReplaceRealtimeBindingIdle) /\ WF_vars(meerkat_ReplaceRealtimeBindingAttached) /\ WF_vars(meerkat_ReplaceRealtimeBindingRunning) /\ WF_vars(meerkat_ReplaceRealtimeBindingRetired) /\ WF_vars(meerkat_ReplaceRealtimeBindingStopped) /\ WF_vars(meerkat_DetachRealtimeBindingIdle) /\ WF_vars(meerkat_DetachRealtimeBindingAttached) /\ WF_vars(meerkat_DetachRealtimeBindingRunning) /\ WF_vars(meerkat_DetachRealtimeBindingRetired) /\ WF_vars(meerkat_DetachRealtimeBindingStopped) /\ WF_vars(meerkat_RequireRealtimeReattachIdle) /\ WF_vars(meerkat_RequireRealtimeReattachAttached) /\ WF_vars(meerkat_RequireRealtimeReattachRunning) /\ WF_vars(meerkat_RequireRealtimeReattachRetired) /\ WF_vars(meerkat_RequireRealtimeReattachStopped) /\ WF_vars(\E arg_authority_epoch \in 0..2 : \E arg_next_binding_state \in RealtimeBindingStateValues : meerkat_PublishRealtimeSignalIdle(arg_authority_epoch, arg_next_binding_state)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : \E arg_next_binding_state \in RealtimeBindingStateValues : meerkat_PublishRealtimeSignalAttached(arg_authority_epoch, arg_next_binding_state)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : \E arg_next_binding_state \in RealtimeBindingStateValues : meerkat_PublishRealtimeSignalRunning(arg_authority_epoch, arg_next_binding_state)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : \E arg_next_binding_state \in RealtimeBindingStateValues : meerkat_PublishRealtimeSignalRetired(arg_authority_epoch, arg_next_binding_state)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : \E arg_next_binding_state \in RealtimeBindingStateValues : meerkat_PublishRealtimeSignalStopped(arg_authority_epoch, arg_next_binding_state)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectPendingIdle(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectPendingAttached(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectPendingRunning(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectPendingRetired(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectPendingStopped(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectedIdle(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectedAttached(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectedRunning(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectedRetired(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectedStopped(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : \E arg_error \in StringValues : meerkat_McpServerFailedIdle(arg_server_id, arg_error)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : \E arg_error \in StringValues : meerkat_McpServerFailedAttached(arg_server_id, arg_error)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : \E arg_error \in StringValues : meerkat_McpServerFailedRunning(arg_server_id, arg_error)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : \E arg_error \in StringValues : meerkat_McpServerFailedRetired(arg_server_id, arg_error)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : \E arg_error \in StringValues : meerkat_McpServerFailedStopped(arg_server_id, arg_error)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerDisconnectedIdle(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerDisconnectedAttached(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerDisconnectedRunning(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerDisconnectedRetired(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerDisconnectedStopped(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerReloadIdle(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerReloadAttached(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerReloadRunning(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerReloadRetired(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerReloadStopped(arg_server_id)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : meerkat_BeginLiveTopologyReconfigureIdle(arg_authority_epoch)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : meerkat_BeginLiveTopologyReconfigureAttached(arg_authority_epoch)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : meerkat_BeginLiveTopologyReconfigureRunning(arg_authority_epoch)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : meerkat_BeginLiveTopologyReconfigureRetired(arg_authority_epoch)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : meerkat_BeginLiveTopologyReconfigureStopped(arg_authority_epoch)) /\ WF_vars(meerkat_MarkLiveTopologyDetachedIdle) /\ WF_vars(meerkat_MarkLiveTopologyDetachedAttached) /\ WF_vars(meerkat_MarkLiveTopologyDetachedRunning) /\ WF_vars(meerkat_MarkLiveTopologyDetachedRetired) /\ WF_vars(meerkat_MarkLiveTopologyDetachedStopped) /\ WF_vars(meerkat_ApplyLiveTopologyIdentityIdle) /\ WF_vars(meerkat_ApplyLiveTopologyIdentityAttached) /\ WF_vars(meerkat_ApplyLiveTopologyIdentityRunning) /\ WF_vars(meerkat_ApplyLiveTopologyIdentityRetired) /\ WF_vars(meerkat_ApplyLiveTopologyIdentityStopped) /\ WF_vars(meerkat_ApplyLiveTopologyVisibilityIdle) /\ WF_vars(meerkat_ApplyLiveTopologyVisibilityAttached) /\ WF_vars(meerkat_ApplyLiveTopologyVisibilityRunning) /\ WF_vars(meerkat_ApplyLiveTopologyVisibilityRetired) /\ WF_vars(meerkat_ApplyLiveTopologyVisibilityStopped) /\ WF_vars(meerkat_CompleteLiveTopologyIdle) /\ WF_vars(meerkat_CompleteLiveTopologyAttached) /\ WF_vars(meerkat_CompleteLiveTopologyRunning) /\ WF_vars(meerkat_CompleteLiveTopologyRetired) /\ WF_vars(meerkat_CompleteLiveTopologyStopped) /\ WF_vars(meerkat_AbortLiveTopologyBeforeDetachIdle) /\ WF_vars(meerkat_AbortLiveTopologyBeforeDetachAttached) /\ WF_vars(meerkat_AbortLiveTopologyBeforeDetachRunning) /\ WF_vars(meerkat_AbortLiveTopologyBeforeDetachRetired) /\ WF_vars(meerkat_AbortLiveTopologyBeforeDetachStopped) /\ WF_vars(meerkat_FailLiveTopologyAfterDetachIdle) /\ WF_vars(meerkat_FailLiveTopologyAfterDetachAttached) /\ WF_vars(meerkat_FailLiveTopologyAfterDetachRunning) /\ WF_vars(meerkat_FailLiveTopologyAfterDetachRetired) /\ WF_vars(meerkat_FailLiveTopologyAfterDetachStopped) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : \E arg_external_addressable \in BOOLEAN : mob_SpawnRunning(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation, arg_external_addressable)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeReady(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : mob_SubmitWorkRunningExternal(arg_agent_runtime_id, arg_fence_token, arg_work_id, arg_origin)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : mob_SubmitWorkRunningInternal(arg_agent_runtime_id, arg_fence_token, arg_work_id, arg_origin)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_RetireMember(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeRetired(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : \E arg_external_addressable \in BOOLEAN : mob_ResetMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation, arg_external_addressable)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : \E arg_external_addressable \in BOOLEAN : mob_RespawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation, arg_external_addressable)) /\ WF_vars(mob_MarkCompleted) /\ WF_vars(mob_DestroyMob) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeDestroyed(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(mob_RecordOperatorActionProvenanceRunning) /\ WF_vars(mob_RecordOperatorActionProvenanceStopped) /\ WF_vars(mob_RecordOperatorActionProvenanceCompleted) /\ WF_vars(mob_RecordOperatorActionProvenanceDestroyed) /\ WF_vars(mob_SetSpawnPolicyRunning) /\ WF_vars(mob_SetSpawnPolicyStopped) /\ WF_vars(mob_SetSpawnPolicyCompleted) /\ WF_vars(mob_SetSpawnPolicyDestroyed) /\ WF_vars(mob_StopRunning) /\ WF_vars(mob_ResumeStopped) /\ WF_vars(mob_CompleteRunning) /\ WF_vars(mob_ResetToRunning) /\ WF_vars(mob_WireRunning) /\ WF_vars(mob_ExternalTurnRunning) /\ WF_vars(mob_InternalTurnRunning) /\ WF_vars(\E arg_task_id \in TaskIdValues : \E arg_task_payload \in MobTaskValues : mob_TaskCreateRunning(arg_task_id, arg_task_payload)) /\ WF_vars(\E arg_task_id \in TaskIdValues : \E arg_new_status \in TaskStatusValues : mob_TaskUpdateRunningPending(arg_task_id, arg_new_status)) /\ WF_vars(\E arg_task_id \in TaskIdValues : \E arg_new_status \in TaskStatusValues : mob_TaskUpdateRunningInProgress(arg_task_id, arg_new_status)) /\ WF_vars(\E arg_task_id \in TaskIdValues : \E arg_new_status \in TaskStatusValues : mob_TaskUpdateRunningCompleted(arg_task_id, arg_new_status)) /\ WF_vars(\E arg_task_id \in TaskIdValues : \E arg_new_status \in TaskStatusValues : mob_TaskUpdateRunningCancelled(arg_task_id, arg_new_status)) /\ WF_vars(mob_ForceCancelRunning) /\ WF_vars(mob_SubscribeAgentEventsRunning) /\ WF_vars(mob_SubscribeAgentEventsStopped) /\ WF_vars(mob_SubscribeAgentEventsCompleted) /\ WF_vars(mob_SubscribeAgentEventsDestroyed) /\ WF_vars(mob_SubscribeAllAgentEventsRunning) /\ WF_vars(mob_SubscribeAllAgentEventsStopped) /\ WF_vars(mob_SubscribeAllAgentEventsCompleted) /\ WF_vars(mob_SubscribeAllAgentEventsDestroyed) /\ WF_vars(mob_SubscribeMobEventsRunning) /\ WF_vars(mob_SubscribeMobEventsStopped) /\ WF_vars(mob_SubscribeMobEventsCompleted) /\ WF_vars(mob_SubscribeMobEventsDestroyed) /\ WF_vars(mob_ShutdownRunning) /\ WF_vars(mob_ShutdownStopped) /\ WF_vars(mob_ShutdownCompleted) /\ WF_vars(mob_CancelFlowRunning) /\ WF_vars(mob_InitializeOrchestratorRunning) /\ WF_vars(mob_BindCoordinatorRunning) /\ WF_vars(mob_UnbindCoordinatorRunning) /\ WF_vars(mob_StageSpawnRunning) /\ WF_vars(mob_StopOrchestratorRunning) /\ WF_vars(mob_StopOrchestratorStopped) /\ WF_vars(mob_StopOrchestratorCompleted) /\ WF_vars(mob_ResumeOrchestratorRunning) /\ WF_vars(mob_ResumeOrchestratorStopped) /\ WF_vars(mob_ResumeOrchestratorCompleted) /\ WF_vars(mob_DestroyOrchestratorRunning) /\ WF_vars(mob_DestroyOrchestratorStopped) /\ WF_vars(mob_DestroyOrchestratorCompleted) /\ WF_vars(mob_ForceCancelMemberRunning) /\ WF_vars(mob_MemberPeerExposedRunning) /\ WF_vars(mob_MemberTerminalizedRunning) /\ WF_vars(mob_OperationPeerTrustedRunning) /\ WF_vars(mob_PeerInputAdmittedRunning) /\ WF_vars(mob_BeginCleanupStopped) /\ WF_vars(mob_BeginCleanupCompleted) /\ WF_vars(mob_FinishCleanupStopped) /\ WF_vars(mob_FinishCleanupCompleted) /\ WF_vars(mob_RunFlowRunning) /\ WF_vars(mob_StartFlowRunning) /\ WF_vars(mob_CreateRunRunning) /\ WF_vars(mob_StartRunRunning) /\ WF_vars(mob_UnwireRunning) /\ WF_vars(mob_CompleteFlowRunning) /\ WF_vars(mob_FinishRunRunning) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireRunning(arg_agent_runtime_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireStopped(arg_agent_runtime_id)) /\ WF_vars(mob_RetireAllRunning) /\ WF_vars(mob_RetireAllStopped) /\ WF_vars(mob_CompleteSpawnRunning) /\ WF_vars(mob_DestroyFromAny) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RespawnRunning(arg_agent_runtime_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_CancelAllWorkRunning(arg_agent_runtime_id, arg_fence_token))
WitnessSpec_retire_runtime_path == WitnessInit_retire_runtime_path /\ [] [WitnessNext_retire_runtime_path]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(RejectPendingEntryInput) /\ WF_vars(meerkat_Initialize) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSessionIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSessionAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSessionRunning(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSessionRetired(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSessionStopped(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSessionIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSessionAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSessionRunning(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSessionRetired(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSessionStopped(arg_session_id)) /\ WF_vars(\E arg_previous_identity \in SessionLlmIdentityValues : \E arg_previous_visibility_state \in SessionToolVisibilityStateValues : \E arg_previous_capability_surface \in OptionSessionLlmCapabilitySurfaceValues : \E arg_previous_capability_surface_status \in SessionLlmCapabilitySurfaceStatusValues : \E arg_target_identity \in SessionLlmIdentityValues : \E arg_target_capability_surface \in SessionLlmCapabilitySurfaceValues : \E arg_next_visibility_state \in SessionToolVisibilityStateValues : \E arg_next_capability_base_filter \in ToolFilterValues : \E arg_next_active_visibility_revision \in 0..2 : \E arg_tool_visibility_delta \in SessionToolVisibilityDeltaValues : meerkat_ReconfigureSessionLlmIdentityAttached(arg_previous_identity, arg_previous_visibility_state, arg_previous_capability_surface, arg_previous_capability_surface_status, arg_target_identity, arg_target_capability_surface, arg_next_visibility_state, arg_next_capability_base_filter, arg_next_active_visibility_revision, arg_tool_visibility_delta)) /\ WF_vars(\E arg_previous_identity \in SessionLlmIdentityValues : \E arg_previous_visibility_state \in SessionToolVisibilityStateValues : \E arg_previous_capability_surface \in OptionSessionLlmCapabilitySurfaceValues : \E arg_previous_capability_surface_status \in SessionLlmCapabilitySurfaceStatusValues : \E arg_target_identity \in SessionLlmIdentityValues : \E arg_target_capability_surface \in SessionLlmCapabilitySurfaceValues : \E arg_next_visibility_state \in SessionToolVisibilityStateValues : \E arg_next_capability_base_filter \in ToolFilterValues : \E arg_next_active_visibility_revision \in 0..2 : \E arg_tool_visibility_delta \in SessionToolVisibilityDeltaValues : meerkat_ReconfigureSessionLlmIdentityRunning(arg_previous_identity, arg_previous_visibility_state, arg_previous_capability_surface, arg_previous_capability_surface_status, arg_target_identity, arg_target_capability_surface, arg_next_visibility_state, arg_next_capability_base_filter, arg_next_active_visibility_revision, arg_tool_visibility_delta)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterIdle(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterAttached(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterRunning(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterRetired(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterStopped(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsIdle(arg_names, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsAttached(arg_names, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsRunning(arg_names, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsRetired(arg_names, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsStopped(arg_names, arg_witnesses)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsInitializing(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsIdle(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsAttached(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsRunning(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsRetired(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsStopped(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextIdle(arg_keep_alive)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextAttached(arg_keep_alive)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextRunning(arg_keep_alive)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextRetired(arg_keep_alive)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextStopped(arg_keep_alive)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedIdle(arg_reason)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedAttached(arg_reason)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedRunning(arg_reason)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedRetired(arg_reason)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedStopped(arg_reason)) /\ WF_vars(meerkat_InterruptCurrentRunAttached) /\ WF_vars(meerkat_InterruptCurrentRun) /\ WF_vars(meerkat_CancelAfterBoundaryAttached) /\ WF_vars(meerkat_CancelAfterBoundary) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_BoundaryAppliedPublish(arg_revision)) /\ WF_vars(\E arg_active_filter \in ToolFilterValues : \E arg_staged_filter \in ToolFilterValues : \E arg_active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : meerkat_PublishCommittedVisibleSetIdle(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)) /\ WF_vars(\E arg_active_filter \in ToolFilterValues : \E arg_staged_filter \in ToolFilterValues : \E arg_active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : meerkat_PublishCommittedVisibleSetAttached(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)) /\ WF_vars(\E arg_active_filter \in ToolFilterValues : \E arg_staged_filter \in ToolFilterValues : \E arg_active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : meerkat_PublishCommittedVisibleSetRunning(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)) /\ WF_vars(\E arg_active_filter \in ToolFilterValues : \E arg_staged_filter \in ToolFilterValues : \E arg_active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : meerkat_PublishCommittedVisibleSetRetired(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)) /\ WF_vars(\E arg_active_filter \in ToolFilterValues : \E arg_staged_filter \in ToolFilterValues : \E arg_active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : meerkat_PublishCommittedVisibleSetStopped(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)) /\ WF_vars(meerkat_RetireRequestedFromIdle) /\ WF_vars(meerkat_Reset) /\ WF_vars(meerkat_StopRuntimeExecutorUnbound) /\ WF_vars(meerkat_StopRuntimeExecutorAttached) /\ WF_vars(meerkat_StopRuntimeExecutorRunning) /\ WF_vars(meerkat_Destroy) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorRunning(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorRetired(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorStopped(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsIdle(arg_session_id, arg_intents)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsAttached(arg_session_id, arg_intents)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsRunning(arg_session_id, arg_intents)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsRetired(arg_session_id, arg_intents)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsStopped(arg_session_id, arg_intents)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortRunning(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortRetired(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortStopped(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitRunning(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitRetired(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitStopped(arg_session_id)) /\ WF_vars(meerkat_AbortAllIdle) /\ WF_vars(meerkat_AbortAllAttached) /\ WF_vars(meerkat_AbortAllRunning) /\ WF_vars(meerkat_AbortAllRetired) /\ WF_vars(meerkat_AbortAllStopped) /\ WF_vars(meerkat_EnsureDrainRunningAttached) /\ WF_vars(meerkat_EnsureDrainRunningRunning) /\ WF_vars(\E arg_runtime_id \in AgentRuntimeIdValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : meerkat_IngestIdle(arg_runtime_id, arg_work_id, arg_origin)) /\ WF_vars(\E arg_runtime_id \in AgentRuntimeIdValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : meerkat_IngestAttached(arg_runtime_id, arg_work_id, arg_origin)) /\ WF_vars(\E arg_runtime_id \in AgentRuntimeIdValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : meerkat_IngestRunning(arg_runtime_id, arg_work_id, arg_origin)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventIdle(arg_kind)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventAttached(arg_kind)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventRunning(arg_kind)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventRetired(arg_kind)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventStopped(arg_kind)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionIdleQueued(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionIdleImmediate(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionAttachedImmediate(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionAttachedQueued(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionRunningQueuedPassive(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionRunningQueuedWakeIfIdle(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionRunningInterruptYielding(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionRunningImmediate(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeIdle(arg_input_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeAttached(arg_input_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeRunning(arg_input_id)) /\ WF_vars(meerkat_ClassifyExternalEnvelopeAttached) /\ WF_vars(meerkat_ClassifyExternalEnvelopeRunning) /\ WF_vars(meerkat_ClassifyPlainEventAttached) /\ WF_vars(meerkat_ClassifyPlainEventRunning) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_run_id \in RunIdValues : meerkat_PrepareIdle(arg_session_id, arg_run_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_run_id \in RunIdValues : meerkat_PrepareAttached(arg_session_id, arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : meerkat_DrainQueuedRunRetired(arg_run_id)) /\ WF_vars(meerkat_StartConversationRunAttached) /\ WF_vars(meerkat_StartImmediateAppendAttached) /\ WF_vars(meerkat_StartImmediateContextAttached) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_run_id \in RunIdValues : meerkat_CommitRunningToIdle(arg_input_id, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_run_id \in RunIdValues : meerkat_CommitRunningToAttached(arg_input_id, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_run_id \in RunIdValues : meerkat_CommitRunningToRetired(arg_input_id, arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : meerkat_FailRunningToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : meerkat_FailRunningToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : meerkat_FailRunningToRetired(arg_run_id)) /\ WF_vars(meerkat_StageAddAttached) /\ WF_vars(meerkat_StageAddRunning) /\ WF_vars(meerkat_StageRemoveAttached) /\ WF_vars(meerkat_StageRemoveRunning) /\ WF_vars(meerkat_StageReloadAttached) /\ WF_vars(meerkat_StageReloadRunning) /\ WF_vars(meerkat_ApplySurfaceBoundaryAttached) /\ WF_vars(meerkat_ApplySurfaceBoundaryRunning) /\ WF_vars(meerkat_PendingSucceededAttached) /\ WF_vars(meerkat_PendingSucceededRunning) /\ WF_vars(meerkat_PendingFailedAttached) /\ WF_vars(meerkat_PendingFailedRunning) /\ WF_vars(meerkat_CallStartedAttached) /\ WF_vars(meerkat_CallStartedRunning) /\ WF_vars(meerkat_CallFinishedAttached) /\ WF_vars(meerkat_CallFinishedRunning) /\ WF_vars(meerkat_FinalizeRemovalCleanAttached) /\ WF_vars(meerkat_FinalizeRemovalCleanRunning) /\ WF_vars(meerkat_FinalizeRemovalForcedAttached) /\ WF_vars(meerkat_FinalizeRemovalForcedRunning) /\ WF_vars(meerkat_SnapshotAlignedAttached) /\ WF_vars(meerkat_SnapshotAlignedRunning) /\ WF_vars(meerkat_ShutdownSurfaceAttached) /\ WF_vars(meerkat_ShutdownSurfaceRunning) /\ WF_vars(meerkat_RecycleFromIdleOrRetired) /\ WF_vars(meerkat_RecycleFromAttached) /\ WF_vars(\E arg_present \in BOOLEAN : meerkat_ProjectRealtimeIntentIdle(arg_present)) /\ WF_vars(\E arg_present \in BOOLEAN : meerkat_ProjectRealtimeIntentAttached(arg_present)) /\ WF_vars(\E arg_present \in BOOLEAN : meerkat_ProjectRealtimeIntentRunning(arg_present)) /\ WF_vars(\E arg_present \in BOOLEAN : meerkat_ProjectRealtimeIntentRetired(arg_present)) /\ WF_vars(\E arg_present \in BOOLEAN : meerkat_ProjectRealtimeIntentStopped(arg_present)) /\ WF_vars(meerkat_BeginRealtimeBindingIdle) /\ WF_vars(meerkat_BeginRealtimeBindingAttached) /\ WF_vars(meerkat_BeginRealtimeBindingRunning) /\ WF_vars(meerkat_BeginRealtimeBindingRetired) /\ WF_vars(meerkat_BeginRealtimeBindingStopped) /\ WF_vars(meerkat_ReplaceRealtimeBindingIdle) /\ WF_vars(meerkat_ReplaceRealtimeBindingAttached) /\ WF_vars(meerkat_ReplaceRealtimeBindingRunning) /\ WF_vars(meerkat_ReplaceRealtimeBindingRetired) /\ WF_vars(meerkat_ReplaceRealtimeBindingStopped) /\ WF_vars(meerkat_DetachRealtimeBindingIdle) /\ WF_vars(meerkat_DetachRealtimeBindingAttached) /\ WF_vars(meerkat_DetachRealtimeBindingRunning) /\ WF_vars(meerkat_DetachRealtimeBindingRetired) /\ WF_vars(meerkat_DetachRealtimeBindingStopped) /\ WF_vars(meerkat_RequireRealtimeReattachIdle) /\ WF_vars(meerkat_RequireRealtimeReattachAttached) /\ WF_vars(meerkat_RequireRealtimeReattachRunning) /\ WF_vars(meerkat_RequireRealtimeReattachRetired) /\ WF_vars(meerkat_RequireRealtimeReattachStopped) /\ WF_vars(\E arg_authority_epoch \in 0..2 : \E arg_next_binding_state \in RealtimeBindingStateValues : meerkat_PublishRealtimeSignalIdle(arg_authority_epoch, arg_next_binding_state)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : \E arg_next_binding_state \in RealtimeBindingStateValues : meerkat_PublishRealtimeSignalAttached(arg_authority_epoch, arg_next_binding_state)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : \E arg_next_binding_state \in RealtimeBindingStateValues : meerkat_PublishRealtimeSignalRunning(arg_authority_epoch, arg_next_binding_state)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : \E arg_next_binding_state \in RealtimeBindingStateValues : meerkat_PublishRealtimeSignalRetired(arg_authority_epoch, arg_next_binding_state)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : \E arg_next_binding_state \in RealtimeBindingStateValues : meerkat_PublishRealtimeSignalStopped(arg_authority_epoch, arg_next_binding_state)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectPendingIdle(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectPendingAttached(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectPendingRunning(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectPendingRetired(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectPendingStopped(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectedIdle(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectedAttached(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectedRunning(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectedRetired(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectedStopped(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : \E arg_error \in StringValues : meerkat_McpServerFailedIdle(arg_server_id, arg_error)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : \E arg_error \in StringValues : meerkat_McpServerFailedAttached(arg_server_id, arg_error)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : \E arg_error \in StringValues : meerkat_McpServerFailedRunning(arg_server_id, arg_error)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : \E arg_error \in StringValues : meerkat_McpServerFailedRetired(arg_server_id, arg_error)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : \E arg_error \in StringValues : meerkat_McpServerFailedStopped(arg_server_id, arg_error)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerDisconnectedIdle(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerDisconnectedAttached(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerDisconnectedRunning(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerDisconnectedRetired(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerDisconnectedStopped(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerReloadIdle(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerReloadAttached(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerReloadRunning(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerReloadRetired(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerReloadStopped(arg_server_id)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : meerkat_BeginLiveTopologyReconfigureIdle(arg_authority_epoch)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : meerkat_BeginLiveTopologyReconfigureAttached(arg_authority_epoch)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : meerkat_BeginLiveTopologyReconfigureRunning(arg_authority_epoch)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : meerkat_BeginLiveTopologyReconfigureRetired(arg_authority_epoch)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : meerkat_BeginLiveTopologyReconfigureStopped(arg_authority_epoch)) /\ WF_vars(meerkat_MarkLiveTopologyDetachedIdle) /\ WF_vars(meerkat_MarkLiveTopologyDetachedAttached) /\ WF_vars(meerkat_MarkLiveTopologyDetachedRunning) /\ WF_vars(meerkat_MarkLiveTopologyDetachedRetired) /\ WF_vars(meerkat_MarkLiveTopologyDetachedStopped) /\ WF_vars(meerkat_ApplyLiveTopologyIdentityIdle) /\ WF_vars(meerkat_ApplyLiveTopologyIdentityAttached) /\ WF_vars(meerkat_ApplyLiveTopologyIdentityRunning) /\ WF_vars(meerkat_ApplyLiveTopologyIdentityRetired) /\ WF_vars(meerkat_ApplyLiveTopologyIdentityStopped) /\ WF_vars(meerkat_ApplyLiveTopologyVisibilityIdle) /\ WF_vars(meerkat_ApplyLiveTopologyVisibilityAttached) /\ WF_vars(meerkat_ApplyLiveTopologyVisibilityRunning) /\ WF_vars(meerkat_ApplyLiveTopologyVisibilityRetired) /\ WF_vars(meerkat_ApplyLiveTopologyVisibilityStopped) /\ WF_vars(meerkat_CompleteLiveTopologyIdle) /\ WF_vars(meerkat_CompleteLiveTopologyAttached) /\ WF_vars(meerkat_CompleteLiveTopologyRunning) /\ WF_vars(meerkat_CompleteLiveTopologyRetired) /\ WF_vars(meerkat_CompleteLiveTopologyStopped) /\ WF_vars(meerkat_AbortLiveTopologyBeforeDetachIdle) /\ WF_vars(meerkat_AbortLiveTopologyBeforeDetachAttached) /\ WF_vars(meerkat_AbortLiveTopologyBeforeDetachRunning) /\ WF_vars(meerkat_AbortLiveTopologyBeforeDetachRetired) /\ WF_vars(meerkat_AbortLiveTopologyBeforeDetachStopped) /\ WF_vars(meerkat_FailLiveTopologyAfterDetachIdle) /\ WF_vars(meerkat_FailLiveTopologyAfterDetachAttached) /\ WF_vars(meerkat_FailLiveTopologyAfterDetachRunning) /\ WF_vars(meerkat_FailLiveTopologyAfterDetachRetired) /\ WF_vars(meerkat_FailLiveTopologyAfterDetachStopped) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : \E arg_external_addressable \in BOOLEAN : mob_SpawnRunning(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation, arg_external_addressable)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeReady(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : mob_SubmitWorkRunningExternal(arg_agent_runtime_id, arg_fence_token, arg_work_id, arg_origin)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : mob_SubmitWorkRunningInternal(arg_agent_runtime_id, arg_fence_token, arg_work_id, arg_origin)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_RetireMember(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeRetired(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : \E arg_external_addressable \in BOOLEAN : mob_ResetMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation, arg_external_addressable)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : \E arg_external_addressable \in BOOLEAN : mob_RespawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation, arg_external_addressable)) /\ WF_vars(mob_MarkCompleted) /\ WF_vars(mob_DestroyMob) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeDestroyed(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(mob_RecordOperatorActionProvenanceRunning) /\ WF_vars(mob_RecordOperatorActionProvenanceStopped) /\ WF_vars(mob_RecordOperatorActionProvenanceCompleted) /\ WF_vars(mob_RecordOperatorActionProvenanceDestroyed) /\ WF_vars(mob_SetSpawnPolicyRunning) /\ WF_vars(mob_SetSpawnPolicyStopped) /\ WF_vars(mob_SetSpawnPolicyCompleted) /\ WF_vars(mob_SetSpawnPolicyDestroyed) /\ WF_vars(mob_StopRunning) /\ WF_vars(mob_ResumeStopped) /\ WF_vars(mob_CompleteRunning) /\ WF_vars(mob_ResetToRunning) /\ WF_vars(mob_WireRunning) /\ WF_vars(mob_ExternalTurnRunning) /\ WF_vars(mob_InternalTurnRunning) /\ WF_vars(\E arg_task_id \in TaskIdValues : \E arg_task_payload \in MobTaskValues : mob_TaskCreateRunning(arg_task_id, arg_task_payload)) /\ WF_vars(\E arg_task_id \in TaskIdValues : \E arg_new_status \in TaskStatusValues : mob_TaskUpdateRunningPending(arg_task_id, arg_new_status)) /\ WF_vars(\E arg_task_id \in TaskIdValues : \E arg_new_status \in TaskStatusValues : mob_TaskUpdateRunningInProgress(arg_task_id, arg_new_status)) /\ WF_vars(\E arg_task_id \in TaskIdValues : \E arg_new_status \in TaskStatusValues : mob_TaskUpdateRunningCompleted(arg_task_id, arg_new_status)) /\ WF_vars(\E arg_task_id \in TaskIdValues : \E arg_new_status \in TaskStatusValues : mob_TaskUpdateRunningCancelled(arg_task_id, arg_new_status)) /\ WF_vars(mob_ForceCancelRunning) /\ WF_vars(mob_SubscribeAgentEventsRunning) /\ WF_vars(mob_SubscribeAgentEventsStopped) /\ WF_vars(mob_SubscribeAgentEventsCompleted) /\ WF_vars(mob_SubscribeAgentEventsDestroyed) /\ WF_vars(mob_SubscribeAllAgentEventsRunning) /\ WF_vars(mob_SubscribeAllAgentEventsStopped) /\ WF_vars(mob_SubscribeAllAgentEventsCompleted) /\ WF_vars(mob_SubscribeAllAgentEventsDestroyed) /\ WF_vars(mob_SubscribeMobEventsRunning) /\ WF_vars(mob_SubscribeMobEventsStopped) /\ WF_vars(mob_SubscribeMobEventsCompleted) /\ WF_vars(mob_SubscribeMobEventsDestroyed) /\ WF_vars(mob_ShutdownRunning) /\ WF_vars(mob_ShutdownStopped) /\ WF_vars(mob_ShutdownCompleted) /\ WF_vars(mob_CancelFlowRunning) /\ WF_vars(mob_InitializeOrchestratorRunning) /\ WF_vars(mob_BindCoordinatorRunning) /\ WF_vars(mob_UnbindCoordinatorRunning) /\ WF_vars(mob_StageSpawnRunning) /\ WF_vars(mob_StopOrchestratorRunning) /\ WF_vars(mob_StopOrchestratorStopped) /\ WF_vars(mob_StopOrchestratorCompleted) /\ WF_vars(mob_ResumeOrchestratorRunning) /\ WF_vars(mob_ResumeOrchestratorStopped) /\ WF_vars(mob_ResumeOrchestratorCompleted) /\ WF_vars(mob_DestroyOrchestratorRunning) /\ WF_vars(mob_DestroyOrchestratorStopped) /\ WF_vars(mob_DestroyOrchestratorCompleted) /\ WF_vars(mob_ForceCancelMemberRunning) /\ WF_vars(mob_MemberPeerExposedRunning) /\ WF_vars(mob_MemberTerminalizedRunning) /\ WF_vars(mob_OperationPeerTrustedRunning) /\ WF_vars(mob_PeerInputAdmittedRunning) /\ WF_vars(mob_BeginCleanupStopped) /\ WF_vars(mob_BeginCleanupCompleted) /\ WF_vars(mob_FinishCleanupStopped) /\ WF_vars(mob_FinishCleanupCompleted) /\ WF_vars(mob_RunFlowRunning) /\ WF_vars(mob_StartFlowRunning) /\ WF_vars(mob_CreateRunRunning) /\ WF_vars(mob_StartRunRunning) /\ WF_vars(mob_UnwireRunning) /\ WF_vars(mob_CompleteFlowRunning) /\ WF_vars(mob_FinishRunRunning) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireRunning(arg_agent_runtime_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireStopped(arg_agent_runtime_id)) /\ WF_vars(mob_RetireAllRunning) /\ WF_vars(mob_RetireAllStopped) /\ WF_vars(mob_CompleteSpawnRunning) /\ WF_vars(mob_DestroyFromAny) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RespawnRunning(arg_agent_runtime_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_CancelAllWorkRunning(arg_agent_runtime_id, arg_fence_token))
WitnessSpec_destroy_runtime_path == WitnessInit_destroy_runtime_path /\ [] [WitnessNext_destroy_runtime_path]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(RejectPendingEntryInput) /\ WF_vars(meerkat_Initialize) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSessionIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSessionAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSessionRunning(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSessionRetired(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSessionStopped(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSessionIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSessionAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSessionRunning(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSessionRetired(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSessionStopped(arg_session_id)) /\ WF_vars(\E arg_previous_identity \in SessionLlmIdentityValues : \E arg_previous_visibility_state \in SessionToolVisibilityStateValues : \E arg_previous_capability_surface \in OptionSessionLlmCapabilitySurfaceValues : \E arg_previous_capability_surface_status \in SessionLlmCapabilitySurfaceStatusValues : \E arg_target_identity \in SessionLlmIdentityValues : \E arg_target_capability_surface \in SessionLlmCapabilitySurfaceValues : \E arg_next_visibility_state \in SessionToolVisibilityStateValues : \E arg_next_capability_base_filter \in ToolFilterValues : \E arg_next_active_visibility_revision \in 0..2 : \E arg_tool_visibility_delta \in SessionToolVisibilityDeltaValues : meerkat_ReconfigureSessionLlmIdentityAttached(arg_previous_identity, arg_previous_visibility_state, arg_previous_capability_surface, arg_previous_capability_surface_status, arg_target_identity, arg_target_capability_surface, arg_next_visibility_state, arg_next_capability_base_filter, arg_next_active_visibility_revision, arg_tool_visibility_delta)) /\ WF_vars(\E arg_previous_identity \in SessionLlmIdentityValues : \E arg_previous_visibility_state \in SessionToolVisibilityStateValues : \E arg_previous_capability_surface \in OptionSessionLlmCapabilitySurfaceValues : \E arg_previous_capability_surface_status \in SessionLlmCapabilitySurfaceStatusValues : \E arg_target_identity \in SessionLlmIdentityValues : \E arg_target_capability_surface \in SessionLlmCapabilitySurfaceValues : \E arg_next_visibility_state \in SessionToolVisibilityStateValues : \E arg_next_capability_base_filter \in ToolFilterValues : \E arg_next_active_visibility_revision \in 0..2 : \E arg_tool_visibility_delta \in SessionToolVisibilityDeltaValues : meerkat_ReconfigureSessionLlmIdentityRunning(arg_previous_identity, arg_previous_visibility_state, arg_previous_capability_surface, arg_previous_capability_surface_status, arg_target_identity, arg_target_capability_surface, arg_next_visibility_state, arg_next_capability_base_filter, arg_next_active_visibility_revision, arg_tool_visibility_delta)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterIdle(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterAttached(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterRunning(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterRetired(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterStopped(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsIdle(arg_names, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsAttached(arg_names, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsRunning(arg_names, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsRetired(arg_names, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsStopped(arg_names, arg_witnesses)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsInitializing(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsIdle(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsAttached(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsRunning(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsRetired(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindingsStopped(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextIdle(arg_keep_alive)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextAttached(arg_keep_alive)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextRunning(arg_keep_alive)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextRetired(arg_keep_alive)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextStopped(arg_keep_alive)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedIdle(arg_reason)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedAttached(arg_reason)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedRunning(arg_reason)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedRetired(arg_reason)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedStopped(arg_reason)) /\ WF_vars(meerkat_InterruptCurrentRunAttached) /\ WF_vars(meerkat_InterruptCurrentRun) /\ WF_vars(meerkat_CancelAfterBoundaryAttached) /\ WF_vars(meerkat_CancelAfterBoundary) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_BoundaryAppliedPublish(arg_revision)) /\ WF_vars(\E arg_active_filter \in ToolFilterValues : \E arg_staged_filter \in ToolFilterValues : \E arg_active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : meerkat_PublishCommittedVisibleSetIdle(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)) /\ WF_vars(\E arg_active_filter \in ToolFilterValues : \E arg_staged_filter \in ToolFilterValues : \E arg_active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : meerkat_PublishCommittedVisibleSetAttached(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)) /\ WF_vars(\E arg_active_filter \in ToolFilterValues : \E arg_staged_filter \in ToolFilterValues : \E arg_active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : meerkat_PublishCommittedVisibleSetRunning(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)) /\ WF_vars(\E arg_active_filter \in ToolFilterValues : \E arg_staged_filter \in ToolFilterValues : \E arg_active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : meerkat_PublishCommittedVisibleSetRetired(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)) /\ WF_vars(\E arg_active_filter \in ToolFilterValues : \E arg_staged_filter \in ToolFilterValues : \E arg_active_requested_deferred_names \in SetOfStringValues : \E arg_staged_requested_deferred_names \in SetOfStringValues : \E arg_active_visibility_revision \in 0..2 : \E arg_staged_visibility_revision \in 0..2 : meerkat_PublishCommittedVisibleSetStopped(arg_active_filter, arg_staged_filter, arg_active_requested_deferred_names, arg_staged_requested_deferred_names, arg_active_visibility_revision, arg_staged_visibility_revision)) /\ WF_vars(meerkat_RetireRequestedFromIdle) /\ WF_vars(meerkat_Reset) /\ WF_vars(meerkat_StopRuntimeExecutorUnbound) /\ WF_vars(meerkat_StopRuntimeExecutorAttached) /\ WF_vars(meerkat_StopRuntimeExecutorRunning) /\ WF_vars(meerkat_Destroy) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorRunning(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorRetired(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorStopped(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsIdle(arg_session_id, arg_intents)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsAttached(arg_session_id, arg_intents)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsRunning(arg_session_id, arg_intents)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsRetired(arg_session_id, arg_intents)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsStopped(arg_session_id, arg_intents)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortRunning(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortRetired(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortStopped(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitRunning(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitRetired(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitStopped(arg_session_id)) /\ WF_vars(meerkat_AbortAllIdle) /\ WF_vars(meerkat_AbortAllAttached) /\ WF_vars(meerkat_AbortAllRunning) /\ WF_vars(meerkat_AbortAllRetired) /\ WF_vars(meerkat_AbortAllStopped) /\ WF_vars(meerkat_EnsureDrainRunningAttached) /\ WF_vars(meerkat_EnsureDrainRunningRunning) /\ WF_vars(\E arg_runtime_id \in AgentRuntimeIdValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : meerkat_IngestIdle(arg_runtime_id, arg_work_id, arg_origin)) /\ WF_vars(\E arg_runtime_id \in AgentRuntimeIdValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : meerkat_IngestAttached(arg_runtime_id, arg_work_id, arg_origin)) /\ WF_vars(\E arg_runtime_id \in AgentRuntimeIdValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : meerkat_IngestRunning(arg_runtime_id, arg_work_id, arg_origin)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventIdle(arg_kind)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventAttached(arg_kind)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventRunning(arg_kind)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventRetired(arg_kind)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventStopped(arg_kind)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionIdleQueued(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionIdleImmediate(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionAttachedImmediate(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionAttachedQueued(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionRunningQueuedPassive(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionRunningQueuedWakeIfIdle(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionRunningInterruptYielding(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_request_immediate_processing \in BOOLEAN : \E arg_interrupt_yielding \in BOOLEAN : \E arg_wake_if_idle \in BOOLEAN : \E arg_run_id \in RunIdValues : meerkat_AcceptWithCompletionRunningImmediate(arg_input_id, arg_request_immediate_processing, arg_interrupt_yielding, arg_wake_if_idle, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeIdle(arg_input_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeAttached(arg_input_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeRunning(arg_input_id)) /\ WF_vars(meerkat_ClassifyExternalEnvelopeAttached) /\ WF_vars(meerkat_ClassifyExternalEnvelopeRunning) /\ WF_vars(meerkat_ClassifyPlainEventAttached) /\ WF_vars(meerkat_ClassifyPlainEventRunning) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_run_id \in RunIdValues : meerkat_PrepareIdle(arg_session_id, arg_run_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_run_id \in RunIdValues : meerkat_PrepareAttached(arg_session_id, arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : meerkat_DrainQueuedRunRetired(arg_run_id)) /\ WF_vars(meerkat_StartConversationRunAttached) /\ WF_vars(meerkat_StartImmediateAppendAttached) /\ WF_vars(meerkat_StartImmediateContextAttached) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_run_id \in RunIdValues : meerkat_CommitRunningToIdle(arg_input_id, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_run_id \in RunIdValues : meerkat_CommitRunningToAttached(arg_input_id, arg_run_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_run_id \in RunIdValues : meerkat_CommitRunningToRetired(arg_input_id, arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : meerkat_FailRunningToIdle(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : meerkat_FailRunningToAttached(arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : meerkat_FailRunningToRetired(arg_run_id)) /\ WF_vars(meerkat_StageAddAttached) /\ WF_vars(meerkat_StageAddRunning) /\ WF_vars(meerkat_StageRemoveAttached) /\ WF_vars(meerkat_StageRemoveRunning) /\ WF_vars(meerkat_StageReloadAttached) /\ WF_vars(meerkat_StageReloadRunning) /\ WF_vars(meerkat_ApplySurfaceBoundaryAttached) /\ WF_vars(meerkat_ApplySurfaceBoundaryRunning) /\ WF_vars(meerkat_PendingSucceededAttached) /\ WF_vars(meerkat_PendingSucceededRunning) /\ WF_vars(meerkat_PendingFailedAttached) /\ WF_vars(meerkat_PendingFailedRunning) /\ WF_vars(meerkat_CallStartedAttached) /\ WF_vars(meerkat_CallStartedRunning) /\ WF_vars(meerkat_CallFinishedAttached) /\ WF_vars(meerkat_CallFinishedRunning) /\ WF_vars(meerkat_FinalizeRemovalCleanAttached) /\ WF_vars(meerkat_FinalizeRemovalCleanRunning) /\ WF_vars(meerkat_FinalizeRemovalForcedAttached) /\ WF_vars(meerkat_FinalizeRemovalForcedRunning) /\ WF_vars(meerkat_SnapshotAlignedAttached) /\ WF_vars(meerkat_SnapshotAlignedRunning) /\ WF_vars(meerkat_ShutdownSurfaceAttached) /\ WF_vars(meerkat_ShutdownSurfaceRunning) /\ WF_vars(meerkat_RecycleFromIdleOrRetired) /\ WF_vars(meerkat_RecycleFromAttached) /\ WF_vars(\E arg_present \in BOOLEAN : meerkat_ProjectRealtimeIntentIdle(arg_present)) /\ WF_vars(\E arg_present \in BOOLEAN : meerkat_ProjectRealtimeIntentAttached(arg_present)) /\ WF_vars(\E arg_present \in BOOLEAN : meerkat_ProjectRealtimeIntentRunning(arg_present)) /\ WF_vars(\E arg_present \in BOOLEAN : meerkat_ProjectRealtimeIntentRetired(arg_present)) /\ WF_vars(\E arg_present \in BOOLEAN : meerkat_ProjectRealtimeIntentStopped(arg_present)) /\ WF_vars(meerkat_BeginRealtimeBindingIdle) /\ WF_vars(meerkat_BeginRealtimeBindingAttached) /\ WF_vars(meerkat_BeginRealtimeBindingRunning) /\ WF_vars(meerkat_BeginRealtimeBindingRetired) /\ WF_vars(meerkat_BeginRealtimeBindingStopped) /\ WF_vars(meerkat_ReplaceRealtimeBindingIdle) /\ WF_vars(meerkat_ReplaceRealtimeBindingAttached) /\ WF_vars(meerkat_ReplaceRealtimeBindingRunning) /\ WF_vars(meerkat_ReplaceRealtimeBindingRetired) /\ WF_vars(meerkat_ReplaceRealtimeBindingStopped) /\ WF_vars(meerkat_DetachRealtimeBindingIdle) /\ WF_vars(meerkat_DetachRealtimeBindingAttached) /\ WF_vars(meerkat_DetachRealtimeBindingRunning) /\ WF_vars(meerkat_DetachRealtimeBindingRetired) /\ WF_vars(meerkat_DetachRealtimeBindingStopped) /\ WF_vars(meerkat_RequireRealtimeReattachIdle) /\ WF_vars(meerkat_RequireRealtimeReattachAttached) /\ WF_vars(meerkat_RequireRealtimeReattachRunning) /\ WF_vars(meerkat_RequireRealtimeReattachRetired) /\ WF_vars(meerkat_RequireRealtimeReattachStopped) /\ WF_vars(\E arg_authority_epoch \in 0..2 : \E arg_next_binding_state \in RealtimeBindingStateValues : meerkat_PublishRealtimeSignalIdle(arg_authority_epoch, arg_next_binding_state)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : \E arg_next_binding_state \in RealtimeBindingStateValues : meerkat_PublishRealtimeSignalAttached(arg_authority_epoch, arg_next_binding_state)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : \E arg_next_binding_state \in RealtimeBindingStateValues : meerkat_PublishRealtimeSignalRunning(arg_authority_epoch, arg_next_binding_state)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : \E arg_next_binding_state \in RealtimeBindingStateValues : meerkat_PublishRealtimeSignalRetired(arg_authority_epoch, arg_next_binding_state)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : \E arg_next_binding_state \in RealtimeBindingStateValues : meerkat_PublishRealtimeSignalStopped(arg_authority_epoch, arg_next_binding_state)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectPendingIdle(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectPendingAttached(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectPendingRunning(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectPendingRetired(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectPendingStopped(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectedIdle(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectedAttached(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectedRunning(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectedRetired(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerConnectedStopped(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : \E arg_error \in StringValues : meerkat_McpServerFailedIdle(arg_server_id, arg_error)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : \E arg_error \in StringValues : meerkat_McpServerFailedAttached(arg_server_id, arg_error)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : \E arg_error \in StringValues : meerkat_McpServerFailedRunning(arg_server_id, arg_error)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : \E arg_error \in StringValues : meerkat_McpServerFailedRetired(arg_server_id, arg_error)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : \E arg_error \in StringValues : meerkat_McpServerFailedStopped(arg_server_id, arg_error)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerDisconnectedIdle(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerDisconnectedAttached(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerDisconnectedRunning(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerDisconnectedRetired(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerDisconnectedStopped(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerReloadIdle(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerReloadAttached(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerReloadRunning(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerReloadRetired(arg_server_id)) /\ WF_vars(\E arg_server_id \in McpServerIdValues : meerkat_McpServerReloadStopped(arg_server_id)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : meerkat_BeginLiveTopologyReconfigureIdle(arg_authority_epoch)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : meerkat_BeginLiveTopologyReconfigureAttached(arg_authority_epoch)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : meerkat_BeginLiveTopologyReconfigureRunning(arg_authority_epoch)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : meerkat_BeginLiveTopologyReconfigureRetired(arg_authority_epoch)) /\ WF_vars(\E arg_authority_epoch \in 0..2 : meerkat_BeginLiveTopologyReconfigureStopped(arg_authority_epoch)) /\ WF_vars(meerkat_MarkLiveTopologyDetachedIdle) /\ WF_vars(meerkat_MarkLiveTopologyDetachedAttached) /\ WF_vars(meerkat_MarkLiveTopologyDetachedRunning) /\ WF_vars(meerkat_MarkLiveTopologyDetachedRetired) /\ WF_vars(meerkat_MarkLiveTopologyDetachedStopped) /\ WF_vars(meerkat_ApplyLiveTopologyIdentityIdle) /\ WF_vars(meerkat_ApplyLiveTopologyIdentityAttached) /\ WF_vars(meerkat_ApplyLiveTopologyIdentityRunning) /\ WF_vars(meerkat_ApplyLiveTopologyIdentityRetired) /\ WF_vars(meerkat_ApplyLiveTopologyIdentityStopped) /\ WF_vars(meerkat_ApplyLiveTopologyVisibilityIdle) /\ WF_vars(meerkat_ApplyLiveTopologyVisibilityAttached) /\ WF_vars(meerkat_ApplyLiveTopologyVisibilityRunning) /\ WF_vars(meerkat_ApplyLiveTopologyVisibilityRetired) /\ WF_vars(meerkat_ApplyLiveTopologyVisibilityStopped) /\ WF_vars(meerkat_CompleteLiveTopologyIdle) /\ WF_vars(meerkat_CompleteLiveTopologyAttached) /\ WF_vars(meerkat_CompleteLiveTopologyRunning) /\ WF_vars(meerkat_CompleteLiveTopologyRetired) /\ WF_vars(meerkat_CompleteLiveTopologyStopped) /\ WF_vars(meerkat_AbortLiveTopologyBeforeDetachIdle) /\ WF_vars(meerkat_AbortLiveTopologyBeforeDetachAttached) /\ WF_vars(meerkat_AbortLiveTopologyBeforeDetachRunning) /\ WF_vars(meerkat_AbortLiveTopologyBeforeDetachRetired) /\ WF_vars(meerkat_AbortLiveTopologyBeforeDetachStopped) /\ WF_vars(meerkat_FailLiveTopologyAfterDetachIdle) /\ WF_vars(meerkat_FailLiveTopologyAfterDetachAttached) /\ WF_vars(meerkat_FailLiveTopologyAfterDetachRunning) /\ WF_vars(meerkat_FailLiveTopologyAfterDetachRetired) /\ WF_vars(meerkat_FailLiveTopologyAfterDetachStopped) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : \E arg_external_addressable \in BOOLEAN : mob_SpawnRunning(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation, arg_external_addressable)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeReady(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : mob_SubmitWorkRunningExternal(arg_agent_runtime_id, arg_fence_token, arg_work_id, arg_origin)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : \E arg_origin \in StringValues : mob_SubmitWorkRunningInternal(arg_agent_runtime_id, arg_fence_token, arg_work_id, arg_origin)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_RetireMember(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeRetired(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : \E arg_external_addressable \in BOOLEAN : mob_ResetMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation, arg_external_addressable)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : \E arg_external_addressable \in BOOLEAN : mob_RespawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation, arg_external_addressable)) /\ WF_vars(mob_MarkCompleted) /\ WF_vars(mob_DestroyMob) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeDestroyed(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(mob_RecordOperatorActionProvenanceRunning) /\ WF_vars(mob_RecordOperatorActionProvenanceStopped) /\ WF_vars(mob_RecordOperatorActionProvenanceCompleted) /\ WF_vars(mob_RecordOperatorActionProvenanceDestroyed) /\ WF_vars(mob_SetSpawnPolicyRunning) /\ WF_vars(mob_SetSpawnPolicyStopped) /\ WF_vars(mob_SetSpawnPolicyCompleted) /\ WF_vars(mob_SetSpawnPolicyDestroyed) /\ WF_vars(mob_StopRunning) /\ WF_vars(mob_ResumeStopped) /\ WF_vars(mob_CompleteRunning) /\ WF_vars(mob_ResetToRunning) /\ WF_vars(mob_WireRunning) /\ WF_vars(mob_ExternalTurnRunning) /\ WF_vars(mob_InternalTurnRunning) /\ WF_vars(\E arg_task_id \in TaskIdValues : \E arg_task_payload \in MobTaskValues : mob_TaskCreateRunning(arg_task_id, arg_task_payload)) /\ WF_vars(\E arg_task_id \in TaskIdValues : \E arg_new_status \in TaskStatusValues : mob_TaskUpdateRunningPending(arg_task_id, arg_new_status)) /\ WF_vars(\E arg_task_id \in TaskIdValues : \E arg_new_status \in TaskStatusValues : mob_TaskUpdateRunningInProgress(arg_task_id, arg_new_status)) /\ WF_vars(\E arg_task_id \in TaskIdValues : \E arg_new_status \in TaskStatusValues : mob_TaskUpdateRunningCompleted(arg_task_id, arg_new_status)) /\ WF_vars(\E arg_task_id \in TaskIdValues : \E arg_new_status \in TaskStatusValues : mob_TaskUpdateRunningCancelled(arg_task_id, arg_new_status)) /\ WF_vars(mob_ForceCancelRunning) /\ WF_vars(mob_SubscribeAgentEventsRunning) /\ WF_vars(mob_SubscribeAgentEventsStopped) /\ WF_vars(mob_SubscribeAgentEventsCompleted) /\ WF_vars(mob_SubscribeAgentEventsDestroyed) /\ WF_vars(mob_SubscribeAllAgentEventsRunning) /\ WF_vars(mob_SubscribeAllAgentEventsStopped) /\ WF_vars(mob_SubscribeAllAgentEventsCompleted) /\ WF_vars(mob_SubscribeAllAgentEventsDestroyed) /\ WF_vars(mob_SubscribeMobEventsRunning) /\ WF_vars(mob_SubscribeMobEventsStopped) /\ WF_vars(mob_SubscribeMobEventsCompleted) /\ WF_vars(mob_SubscribeMobEventsDestroyed) /\ WF_vars(mob_ShutdownRunning) /\ WF_vars(mob_ShutdownStopped) /\ WF_vars(mob_ShutdownCompleted) /\ WF_vars(mob_CancelFlowRunning) /\ WF_vars(mob_InitializeOrchestratorRunning) /\ WF_vars(mob_BindCoordinatorRunning) /\ WF_vars(mob_UnbindCoordinatorRunning) /\ WF_vars(mob_StageSpawnRunning) /\ WF_vars(mob_StopOrchestratorRunning) /\ WF_vars(mob_StopOrchestratorStopped) /\ WF_vars(mob_StopOrchestratorCompleted) /\ WF_vars(mob_ResumeOrchestratorRunning) /\ WF_vars(mob_ResumeOrchestratorStopped) /\ WF_vars(mob_ResumeOrchestratorCompleted) /\ WF_vars(mob_DestroyOrchestratorRunning) /\ WF_vars(mob_DestroyOrchestratorStopped) /\ WF_vars(mob_DestroyOrchestratorCompleted) /\ WF_vars(mob_ForceCancelMemberRunning) /\ WF_vars(mob_MemberPeerExposedRunning) /\ WF_vars(mob_MemberTerminalizedRunning) /\ WF_vars(mob_OperationPeerTrustedRunning) /\ WF_vars(mob_PeerInputAdmittedRunning) /\ WF_vars(mob_BeginCleanupStopped) /\ WF_vars(mob_BeginCleanupCompleted) /\ WF_vars(mob_FinishCleanupStopped) /\ WF_vars(mob_FinishCleanupCompleted) /\ WF_vars(mob_RunFlowRunning) /\ WF_vars(mob_StartFlowRunning) /\ WF_vars(mob_CreateRunRunning) /\ WF_vars(mob_StartRunRunning) /\ WF_vars(mob_UnwireRunning) /\ WF_vars(mob_CompleteFlowRunning) /\ WF_vars(mob_FinishRunRunning) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireRunning(arg_agent_runtime_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireStopped(arg_agent_runtime_id)) /\ WF_vars(mob_RetireAllRunning) /\ WF_vars(mob_RetireAllStopped) /\ WF_vars(mob_CompleteSpawnRunning) /\ WF_vars(mob_DestroyFromAny) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RespawnRunning(arg_agent_runtime_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_CancelAllWorkRunning(arg_agent_runtime_id, arg_fence_token))

WitnessRouteObserved_basic_round_trip_binding_request_reaches_meerkat == <> RouteObserved_binding_request_reaches_meerkat
WitnessRouteObserved_basic_round_trip_work_request_reaches_meerkat == <> RouteObserved_work_request_reaches_meerkat
WitnessRouteObserved_basic_round_trip_runtime_bound_reaches_mob == <> RouteObserved_runtime_bound_reaches_mob
WitnessRouteObserved_retire_runtime_path_retire_request_reaches_meerkat == <> RouteObserved_retire_request_reaches_meerkat
WitnessRouteObserved_retire_runtime_path_runtime_retired_reaches_mob == <> RouteObserved_runtime_retired_reaches_mob
WitnessRouteObserved_destroy_runtime_path_destroy_request_reaches_meerkat == <> RouteObserved_destroy_request_reaches_meerkat
WitnessRouteObserved_destroy_runtime_path_runtime_destroyed_reaches_mob == <> RouteObserved_runtime_destroyed_reaches_mob

THEOREM Spec => []meerkat_fence_requires_bound_runtime
THEOREM Spec => []meerkat_running_has_current_run
THEOREM Spec => []meerkat_current_run_only_while_running_or_retired
THEOREM Spec => []meerkat_realtime_binding_epoch_consistency

=============================================================================
