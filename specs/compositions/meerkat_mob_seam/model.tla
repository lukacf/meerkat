---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for meerkat_mob_seam.

CONSTANTS AgentIdentityValues, AgentRuntimeIdValues, BooleanValues, FenceTokenValues, GenerationValues, InputIdValues, NatValues, PeerReachabilityReasonValues, PeerReachabilityValues, ReachabilityKeyValues, RunIdValues, SessionIdValues, SetOfReachabilityKeyValues, SetOfStringValues, StringValues, ToolFilterValues, ToolVisibilityWitnessValues, WorkIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionAgentIdentityValues == {None} \cup {Some(x) : x \in AgentIdentityValues}
OptionAgentRuntimeIdValues == {None} \cup {Some(x) : x \in AgentRuntimeIdValues}
OptionFenceTokenValues == {None} \cup {Some(x) : x \in FenceTokenValues}
OptionGenerationValues == {None} \cup {Some(x) : x \in GenerationValues}
OptionPeerReachabilityReasonValues == {None} \cup {Some(x) : x \in PeerReachabilityReasonValues}
OptionSessionIdValues == {None} \cup {Some(x) : x \in SessionIdValues}
OptionWorkIdValues == {None} \cup {Some(x) : x \in WorkIdValues}
MapReachabilityKeyOptionPeerReachabilityReasonValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in ReachabilityKeyValues, v \in OptionPeerReachabilityReasonValues }
MapReachabilityKeyPeerReachabilityValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in ReachabilityKeyValues, v \in PeerReachabilityValues }
MapStringToolVisibilityWitnessValues == {[x \in {} |-> None]} \cup { [x \in {k} |-> v] : k \in StringValues, v \in ToolVisibilityWitnessValues }

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
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
    "member_work_reaches_meerkat",
    "retire_request_reaches_meerkat",
    "destroy_request_reaches_meerkat",
    "runtime_bound_reaches_mob",
    "runtime_retired_reaches_mob",
    "runtime_destroyed_reaches_mob",
    "work_completed_reaches_mob",
    "work_failed_reaches_mob",
    "work_cancelled_reaches_mob"
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
      [] route_name = "member_work_reaches_meerkat" -> "mob"
      [] route_name = "retire_request_reaches_meerkat" -> "mob"
      [] route_name = "destroy_request_reaches_meerkat" -> "mob"
      [] route_name = "runtime_bound_reaches_mob" -> "meerkat"
      [] route_name = "runtime_retired_reaches_mob" -> "meerkat"
      [] route_name = "runtime_destroyed_reaches_mob" -> "meerkat"
      [] route_name = "work_completed_reaches_mob" -> "meerkat"
      [] route_name = "work_failed_reaches_mob" -> "meerkat"
      [] route_name = "work_cancelled_reaches_mob" -> "meerkat"
      [] OTHER -> "unknown_machine"

RouteEffect(route_name) ==
    CASE route_name = "binding_request_reaches_meerkat" -> "RequestRuntimeBinding"
      [] route_name = "member_work_reaches_meerkat" -> "SubmitMemberWork"
      [] route_name = "retire_request_reaches_meerkat" -> "RequestRuntimeRetire"
      [] route_name = "destroy_request_reaches_meerkat" -> "RequestRuntimeDestroy"
      [] route_name = "runtime_bound_reaches_mob" -> "RuntimeBound"
      [] route_name = "runtime_retired_reaches_mob" -> "RuntimeRetired"
      [] route_name = "runtime_destroyed_reaches_mob" -> "RuntimeDestroyed"
      [] route_name = "work_completed_reaches_mob" -> "WorkCompleted"
      [] route_name = "work_failed_reaches_mob" -> "WorkFailed"
      [] route_name = "work_cancelled_reaches_mob" -> "WorkCancelled"
      [] OTHER -> "unknown_effect"

RouteTargetMachine(route_name) ==
    CASE route_name = "binding_request_reaches_meerkat" -> "meerkat"
      [] route_name = "member_work_reaches_meerkat" -> "meerkat"
      [] route_name = "retire_request_reaches_meerkat" -> "meerkat"
      [] route_name = "destroy_request_reaches_meerkat" -> "meerkat"
      [] route_name = "runtime_bound_reaches_mob" -> "mob"
      [] route_name = "runtime_retired_reaches_mob" -> "mob"
      [] route_name = "runtime_destroyed_reaches_mob" -> "mob"
      [] route_name = "work_completed_reaches_mob" -> "mob"
      [] route_name = "work_failed_reaches_mob" -> "mob"
      [] route_name = "work_cancelled_reaches_mob" -> "mob"
      [] OTHER -> "unknown_machine"

RouteTargetInput(route_name) ==
    CASE route_name = "binding_request_reaches_meerkat" -> "PrepareBindings"
      [] route_name = "member_work_reaches_meerkat" -> "SubmitMobWork"
      [] route_name = "retire_request_reaches_meerkat" -> "Retire"
      [] route_name = "destroy_request_reaches_meerkat" -> "Destroy"
      [] route_name = "runtime_bound_reaches_mob" -> "ObserveRuntimeReady"
      [] route_name = "runtime_retired_reaches_mob" -> "ObserveRuntimeRetired"
      [] route_name = "runtime_destroyed_reaches_mob" -> "ObserveRuntimeDestroyed"
      [] route_name = "work_completed_reaches_mob" -> "ObserveWorkCompleted"
      [] route_name = "work_failed_reaches_mob" -> "ObserveWorkFailed"
      [] route_name = "work_cancelled_reaches_mob" -> "ObserveWorkCancelled"
      [] OTHER -> "unknown_input"

RouteTargetKind(route_name) ==
    CASE route_name = "binding_request_reaches_meerkat" -> "Input"
      [] route_name = "member_work_reaches_meerkat" -> "Signal"
      [] route_name = "retire_request_reaches_meerkat" -> "Input"
      [] route_name = "destroy_request_reaches_meerkat" -> "Input"
      [] route_name = "runtime_bound_reaches_mob" -> "Signal"
      [] route_name = "runtime_retired_reaches_mob" -> "Signal"
      [] route_name = "runtime_destroyed_reaches_mob" -> "Signal"
      [] route_name = "work_completed_reaches_mob" -> "Signal"
      [] route_name = "work_failed_reaches_mob" -> "Signal"
      [] route_name = "work_cancelled_reaches_mob" -> "Signal"
      [] OTHER -> "Unknown"

RouteDeliveryKind(route_name) ==
    CASE route_name = "binding_request_reaches_meerkat" -> "Immediate"
      [] route_name = "member_work_reaches_meerkat" -> "Immediate"
      [] route_name = "retire_request_reaches_meerkat" -> "Immediate"
      [] route_name = "destroy_request_reaches_meerkat" -> "Immediate"
      [] route_name = "runtime_bound_reaches_mob" -> "Immediate"
      [] route_name = "runtime_retired_reaches_mob" -> "Immediate"
      [] route_name = "runtime_destroyed_reaches_mob" -> "Immediate"
      [] route_name = "work_completed_reaches_mob" -> "Immediate"
      [] route_name = "work_failed_reaches_mob" -> "Immediate"
      [] route_name = "work_cancelled_reaches_mob" -> "Immediate"
      [] OTHER -> "Unknown"

RouteTargetActor(route_name) == ActorOfMachine(RouteTargetMachine(route_name))

VARIABLES meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

RoutePackets == SeqElements(pending_routes) \cup delivered_routes
PendingActors == {ActorOfMachine(packet.machine) : packet \in SeqElements(pending_inputs)}
HigherPriorityReady(actor) == \E priority \in ActorPriorities : /\ priority[2] = actor /\ priority[1] \in PendingActors

BaseInit ==
    /\ meerkat_phase = "Initializing"
    /\ meerkat_session_id = None
    /\ meerkat_active_runtime_id = None
    /\ meerkat_active_fence_token = None
    /\ meerkat_active_generation = None
    /\ meerkat_active_work_id = None
    /\ meerkat_wake_pending = FALSE
    /\ meerkat_process_pending = FALSE
    /\ meerkat_peer_ingress_configured = FALSE
    /\ meerkat_drain_running = FALSE
    /\ meerkat_resolved_peer_keys = {}
    /\ meerkat_peer_reachability = [x \in {} |-> None]
    /\ meerkat_peer_last_reason = [x \in {} |-> None]
    /\ meerkat_interrupt_pending = FALSE
    /\ meerkat_shutdown_pending = FALSE
    /\ meerkat_inherited_base_filter = "All"
    /\ meerkat_active_filter = "All"
    /\ meerkat_staged_filter = "All"
    /\ meerkat_active_requested_deferred_names = {}
    /\ meerkat_staged_requested_deferred_names = {}
    /\ meerkat_requested_witnesses = [x \in {} |-> None]
    /\ meerkat_filter_witnesses = [x \in {} |-> None]
    /\ meerkat_active_visibility_revision = 0
    /\ meerkat_staged_visibility_revision = 0
    /\ meerkat_committed_visibility_revision = 0
    /\ mob_phase = "Creating"
    /\ mob_active_identity = None
    /\ mob_active_runtime_id = None
    /\ mob_active_fence_token = None
    /\ mob_current_generation = None
    /\ mob_inflight_work_id = None
    /\ mob_active_member_count = 0
    /\ mob_active_run_count = 0
    /\ mob_pending_spawn_count = 0
    /\ mob_retiring_member_count = 0
    /\ mob_wiring_edge_count = 0
    /\ mob_task_count = 0
    /\ mob_event_subscription_count = 0
    /\ mob_active_frame_count = 0
    /\ mob_active_loop_count = 0
    /\ mob_coordinator_bound = FALSE
    /\ mob_kickoff_pending = FALSE
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

WitnessInit_work_terminal_variants ==
    /\ BaseInit
    /\ pending_inputs = <<>>
    /\ observed_inputs = {}
    /\ witness_current_script_input = None
    /\ witness_remaining_script_inputs = <<>>

meerkat__FilterWitnessKeys == DOMAIN meerkat_filter_witnesses

meerkat__RequestedWitnessKeys == DOMAIN meerkat_requested_witnesses

meerkat__HasPendingVisibilityPromotion == (meerkat_staged_visibility_revision > meerkat_active_visibility_revision)

RECURSIVE meerkat_StagePersistentFilterAttached_ForEach0_filter_witnesses(_, _, _)
meerkat_StagePersistentFilterAttached_ForEach0_filter_witnesses(acc, remaining, outer_witnesses) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET name == item IN LET next_acc == MapSet(acc, name, (IF name \in DOMAIN outer_witnesses THEN outer_witnesses[name] ELSE "None")) IN meerkat_StagePersistentFilterAttached_ForEach0_filter_witnesses(next_acc, remaining \ {item}, outer_witnesses)

RECURSIVE meerkat_StagePersistentFilterRunning_ForEach1_filter_witnesses(_, _, _)
meerkat_StagePersistentFilterRunning_ForEach1_filter_witnesses(acc, remaining, outer_witnesses) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET name == item IN LET next_acc == MapSet(acc, name, (IF name \in DOMAIN outer_witnesses THEN outer_witnesses[name] ELSE "None")) IN meerkat_StagePersistentFilterRunning_ForEach1_filter_witnesses(next_acc, remaining \ {item}, outer_witnesses)

RECURSIVE meerkat_RequestDeferredToolsAttached_ForEach2_staged_requested_deferred_names(_, _)
meerkat_RequestDeferredToolsAttached_ForEach2_staged_requested_deferred_names(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET name == item IN LET next_acc == (acc \cup {name}) IN meerkat_RequestDeferredToolsAttached_ForEach2_staged_requested_deferred_names(next_acc, remaining \ {item})

RECURSIVE meerkat_RequestDeferredToolsAttached_ForEach3_requested_witnesses(_, _, _)
meerkat_RequestDeferredToolsAttached_ForEach3_requested_witnesses(acc, remaining, outer_witnesses) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET name == item IN LET next_acc == MapSet(acc, name, (IF name \in DOMAIN outer_witnesses THEN outer_witnesses[name] ELSE "None")) IN meerkat_RequestDeferredToolsAttached_ForEach3_requested_witnesses(next_acc, remaining \ {item}, outer_witnesses)

RECURSIVE meerkat_RequestDeferredToolsRunning_ForEach4_staged_requested_deferred_names(_, _)
meerkat_RequestDeferredToolsRunning_ForEach4_staged_requested_deferred_names(acc, remaining) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET name == item IN LET next_acc == (acc \cup {name}) IN meerkat_RequestDeferredToolsRunning_ForEach4_staged_requested_deferred_names(next_acc, remaining \ {item})

RECURSIVE meerkat_RequestDeferredToolsRunning_ForEach5_requested_witnesses(_, _, _)
meerkat_RequestDeferredToolsRunning_ForEach5_requested_witnesses(acc, remaining, outer_witnesses) == IF remaining = {} THEN acc ELSE LET item == CHOOSE x \in remaining : TRUE IN LET name == item IN LET next_acc == MapSet(acc, name, (IF name \in DOMAIN outer_witnesses THEN outer_witnesses[name] ELSE "None")) IN meerkat_RequestDeferredToolsRunning_ForEach5_requested_witnesses(next_acc, remaining \ {item}, outer_witnesses)

meerkat_Initialize ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Initialize"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Initializing"
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "Initialize", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RegisterSession(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RegisterSession"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle" \/ meerkat_phase = "Stopped" \/ meerkat_phase = "Retired"
       /\ meerkat_phase' = "Idle"
       /\ meerkat_session_id' = Some(packet.payload.session_id)
       /\ UNCHANGED << meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RegisterSession", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_UnregisterSession(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "UnregisterSession"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle" \/ meerkat_phase = "Stopped" \/ meerkat_phase = "Retired"
       /\ (meerkat_session_id = Some(packet.payload.session_id))
       /\ meerkat_phase' = "Idle"
       /\ meerkat_session_id' = None
       /\ meerkat_active_runtime_id' = None
       /\ meerkat_active_fence_token' = None
       /\ meerkat_active_generation' = None
       /\ meerkat_active_work_id' = None
       /\ meerkat_wake_pending' = FALSE
       /\ meerkat_process_pending' = FALSE
       /\ meerkat_peer_ingress_configured' = FALSE
       /\ meerkat_drain_running' = FALSE
       /\ meerkat_resolved_peer_keys' = {}
       /\ meerkat_peer_reachability' = [x \in {} |-> None]
       /\ meerkat_peer_last_reason' = [x \in {} |-> None]
       /\ meerkat_interrupt_pending' = FALSE
       /\ meerkat_shutdown_pending' = FALSE
       /\ meerkat_inherited_base_filter' = "All"
       /\ meerkat_active_filter' = "All"
       /\ meerkat_staged_filter' = "All"
       /\ meerkat_active_requested_deferred_names' = {}
       /\ meerkat_staged_requested_deferred_names' = {}
       /\ meerkat_requested_witnesses' = [x \in {} |-> None]
       /\ meerkat_filter_witnesses' = [x \in {} |-> None]
       /\ meerkat_active_visibility_revision' = 0
       /\ meerkat_staged_visibility_revision' = 0
       /\ meerkat_committed_visibility_revision' = 0
       /\ UNCHANGED << mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "UnregisterSession", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
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
       /\ meerkat_staged_filter' = packet.payload.filter
       /\ meerkat_filter_witnesses' = meerkat_StagePersistentFilterAttached_ForEach0_filter_witnesses(meerkat_filter_witnesses, DOMAIN packet.payload.witnesses, packet.payload.witnesses)
       /\ meerkat_staged_visibility_revision' = (meerkat_staged_visibility_revision) + 1
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_active_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ meerkat_staged_filter' = packet.payload.filter
       /\ meerkat_filter_witnesses' = meerkat_StagePersistentFilterRunning_ForEach1_filter_witnesses(meerkat_filter_witnesses, DOMAIN packet.payload.witnesses, packet.payload.witnesses)
       /\ meerkat_staged_visibility_revision' = (meerkat_staged_visibility_revision) + 1
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_active_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StagePersistentFilterRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
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
       /\ meerkat_staged_requested_deferred_names' = meerkat_RequestDeferredToolsAttached_ForEach2_staged_requested_deferred_names(meerkat_staged_requested_deferred_names, packet.payload.names)
       /\ meerkat_requested_witnesses' = meerkat_RequestDeferredToolsAttached_ForEach3_requested_witnesses(meerkat_requested_witnesses, DOMAIN packet.payload.witnesses, packet.payload.witnesses)
       /\ meerkat_staged_visibility_revision' = (meerkat_staged_visibility_revision) + 1
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ meerkat_staged_requested_deferred_names' = meerkat_RequestDeferredToolsRunning_ForEach4_staged_requested_deferred_names(meerkat_staged_requested_deferred_names, packet.payload.names)
       /\ meerkat_requested_witnesses' = meerkat_RequestDeferredToolsRunning_ForEach5_requested_witnesses(meerkat_requested_witnesses, DOMAIN packet.payload.witnesses, packet.payload.witnesses)
       /\ meerkat_staged_visibility_revision' = (meerkat_staged_visibility_revision) + 1
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RequestDeferredToolsRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PrepareBindings(arg_agent_runtime_id, arg_fence_token, arg_generation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PrepareBindings"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.generation = arg_generation
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle" \/ meerkat_phase = "Stopped" \/ meerkat_phase = "Retired"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ meerkat_active_runtime_id' = Some(packet.payload.agent_runtime_id)
       /\ meerkat_active_fence_token' = Some(packet.payload.fence_token)
       /\ meerkat_active_generation' = Some(packet.payload.generation)
       /\ meerkat_interrupt_pending' = FALSE
       /\ meerkat_shutdown_pending' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "mob", variant |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeBound", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeBound", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "runtime_bound_reaches_mob", source_machine |-> "meerkat", effect |-> "RuntimeBound", target_machine |-> "mob", target_input |-> "ObserveRuntimeReady", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None)], actor |-> "mob_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "PrepareBindings"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeBound", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None), generation |-> (IF "value" \in DOMAIN Some(packet.payload.generation) THEN Some(packet.payload.generation)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "PrepareBindings"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PrepareBindings", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
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
       /\ meerkat_peer_ingress_configured' = TRUE
       /\ meerkat_drain_running' = packet.payload.keep_alive
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ meerkat_peer_ingress_configured' = TRUE
       /\ meerkat_drain_running' = packet.payload.keep_alive
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "SetPeerIngressContextRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
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
       /\ meerkat_drain_running' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ meerkat_drain_running' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeNotice", payload |-> [detail |-> "drain exited", kind |-> "drain"], effect_id |-> (model_step_count + 1), source_transition |-> "NotifyDrainExitedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "NotifyDrainExitedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ReconcileResolvedDirectoryAttached(arg_keys, arg_reachability, arg_last_reason) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ReconcileResolvedDirectory"
       /\ packet.payload.keys = arg_keys
       /\ packet.payload.reachability = arg_reachability
       /\ packet.payload.last_reason = arg_last_reason
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (\A k \in DOMAIN packet.payload.reachability : (k \in packet.payload.keys))
       /\ (\A k \in DOMAIN packet.payload.last_reason : (k \in packet.payload.keys))
       /\ meerkat_phase' = "Attached"
       /\ meerkat_resolved_peer_keys' = packet.payload.keys
       /\ meerkat_peer_reachability' = packet.payload.reachability
       /\ meerkat_peer_last_reason' = packet.payload.last_reason
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ReconcileResolvedDirectoryAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ReconcileResolvedDirectoryRunning(arg_keys, arg_reachability, arg_last_reason) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ReconcileResolvedDirectory"
       /\ packet.payload.keys = arg_keys
       /\ packet.payload.reachability = arg_reachability
       /\ packet.payload.last_reason = arg_last_reason
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (\A k \in DOMAIN packet.payload.reachability : (k \in packet.payload.keys))
       /\ (\A k \in DOMAIN packet.payload.last_reason : (k \in packet.payload.keys))
       /\ meerkat_phase' = "Running"
       /\ meerkat_resolved_peer_keys' = packet.payload.keys
       /\ meerkat_peer_reachability' = packet.payload.reachability
       /\ meerkat_peer_last_reason' = packet.payload.last_reason
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ReconcileResolvedDirectoryRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RecordSendSucceededAttached(arg_key) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RecordSendSucceeded"
       /\ packet.payload.key = arg_key
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (packet.payload.key \in meerkat_resolved_peer_keys)
       /\ meerkat_phase' = "Attached"
       /\ meerkat_peer_reachability' = MapSet(meerkat_peer_reachability, packet.payload.key, "Reachable")
       /\ meerkat_peer_last_reason' = MapSet(meerkat_peer_last_reason, packet.payload.key, None)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RecordSendSucceededAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RecordSendSucceededRunning(arg_key) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RecordSendSucceeded"
       /\ packet.payload.key = arg_key
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (packet.payload.key \in meerkat_resolved_peer_keys)
       /\ meerkat_phase' = "Running"
       /\ meerkat_peer_reachability' = MapSet(meerkat_peer_reachability, packet.payload.key, "Reachable")
       /\ meerkat_peer_last_reason' = MapSet(meerkat_peer_last_reason, packet.payload.key, None)
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RecordSendSucceededRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RecordSendFailedAttached(arg_key, arg_reason) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RecordSendFailed"
       /\ packet.payload.key = arg_key
       /\ packet.payload.reason = arg_reason
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (packet.payload.key \in meerkat_resolved_peer_keys)
       /\ meerkat_phase' = "Attached"
       /\ meerkat_peer_reachability' = MapSet(meerkat_peer_reachability, packet.payload.key, "Unreachable")
       /\ meerkat_peer_last_reason' = MapSet(meerkat_peer_last_reason, packet.payload.key, Some(packet.payload.reason))
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RecordSendFailedAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RecordSendFailedRunning(arg_key, arg_reason) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RecordSendFailed"
       /\ packet.payload.key = arg_key
       /\ packet.payload.reason = arg_reason
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (packet.payload.key \in meerkat_resolved_peer_keys)
       /\ meerkat_phase' = "Running"
       /\ meerkat_peer_reachability' = MapSet(meerkat_peer_reachability, packet.payload.key, "Unreachable")
       /\ meerkat_peer_last_reason' = MapSet(meerkat_peer_last_reason, packet.payload.key, Some(packet.payload.reason))
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RecordSendFailedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_BeginRunFromIdle(arg_agent_runtime_id, arg_fence_token, arg_work_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "SubmitMobWork"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.work_id = arg_work_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_active_runtime_id # None)
       /\ meerkat_phase' = "Running"
       /\ meerkat_active_work_id' = Some(packet.payload.work_id)
       /\ meerkat_wake_pending' = TRUE
       /\ meerkat_interrupt_pending' = FALSE
       /\ meerkat_shutdown_pending' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "BeginRunFromIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_InterruptCurrentRun ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "InterruptCurrentRun"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ meerkat_interrupt_pending' = TRUE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "WakeInterrupt", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "InterruptCurrentRun"], [machine |-> "meerkat", variant |-> "RequestCancellationAtBoundary", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "InterruptCurrentRun"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "InterruptCurrentRun", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CancelAfterBoundary ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "CancelAfterBoundary"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ meerkat_shutdown_pending' = TRUE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RequestCancellationAtBoundary", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CancelAfterBoundary"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CancelAfterBoundary", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_BoundaryAppliedPromote(arg_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "BoundaryApplied"
       /\ packet.payload.revision = arg_revision
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ meerkat__HasPendingVisibilityPromotion
       /\ (meerkat_staged_visibility_revision = packet.payload.revision)
       /\ meerkat_phase' = "Running"
       /\ meerkat_active_filter' = meerkat_staged_filter
       /\ meerkat_active_requested_deferred_names' = meerkat_staged_requested_deferred_names
       /\ meerkat_active_visibility_revision' = meerkat_staged_visibility_revision
       /\ meerkat_committed_visibility_revision' = packet.payload.revision
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_staged_filter, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_staged_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "CommittedVisibleSetPublished", payload |-> [revision |-> packet.payload.revision], effect_id |-> (model_step_count + 1), source_transition |-> "BoundaryAppliedPromote"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "BoundaryAppliedPromote", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_BoundaryAppliedNoop(arg_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "BoundaryApplied"
       /\ packet.payload.revision = arg_revision
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ ~(meerkat__HasPendingVisibilityPromotion)
       /\ (packet.payload.revision <= meerkat_active_visibility_revision)
       /\ meerkat_phase' = "Running"
       /\ meerkat_committed_visibility_revision' = packet.payload.revision
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "CommittedVisibleSetPublished", payload |-> [revision |-> packet.payload.revision], effect_id |-> (model_step_count + 1), source_transition |-> "BoundaryAppliedNoop"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "BoundaryAppliedNoop", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PublishCommittedVisibleSetAttached(arg_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PublishCommittedVisibleSet"
       /\ packet.payload.revision = arg_revision
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_active_visibility_revision = packet.payload.revision)
       /\ meerkat_phase' = "Attached"
       /\ meerkat_committed_visibility_revision' = packet.payload.revision
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "CommittedVisibleSetPublished", payload |-> [revision |-> packet.payload.revision], effect_id |-> (model_step_count + 1), source_transition |-> "PublishCommittedVisibleSetAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PublishCommittedVisibleSetAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PublishCommittedVisibleSetRunning(arg_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PublishCommittedVisibleSet"
       /\ packet.payload.revision = arg_revision
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_visibility_revision = packet.payload.revision)
       /\ meerkat_phase' = "Running"
       /\ meerkat_committed_visibility_revision' = packet.payload.revision
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "CommittedVisibleSetPublished", payload |-> [revision |-> packet.payload.revision], effect_id |-> (model_step_count + 1), source_transition |-> "PublishCommittedVisibleSetRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PublishCommittedVisibleSetRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RunCompleted(arg_work_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RunCompleted"
       /\ packet.payload.work_id = arg_work_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id = Some(packet.payload.work_id))
       /\ meerkat_phase' = "Attached"
       /\ meerkat_active_work_id' = None
       /\ meerkat_interrupt_pending' = FALSE
       /\ meerkat_shutdown_pending' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "mob", variant |-> "ObserveWorkCompleted", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None), work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "work_completed_reaches_mob", source_machine |-> "meerkat", source_effect |-> "WorkCompleted", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "ObserveWorkCompleted", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None), work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "work_completed_reaches_mob", source_machine |-> "meerkat", source_effect |-> "WorkCompleted", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "work_completed_reaches_mob", source_machine |-> "meerkat", effect |-> "WorkCompleted", target_machine |-> "mob", target_input |-> "ObserveWorkCompleted", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None), work_id |-> packet.payload.work_id], actor |-> "mob_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "RunCompleted"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "WorkCompleted", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None), generation |-> (IF "value" \in DOMAIN meerkat_active_generation THEN meerkat_active_generation["value"] ELSE None), work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "RunCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RunCompleted", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RunFailed(arg_work_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RunFailed"
       /\ packet.payload.work_id = arg_work_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id = Some(packet.payload.work_id))
       /\ meerkat_phase' = "Attached"
       /\ meerkat_active_work_id' = None
       /\ meerkat_interrupt_pending' = FALSE
       /\ meerkat_shutdown_pending' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "mob", variant |-> "ObserveWorkFailed", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None), work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "work_failed_reaches_mob", source_machine |-> "meerkat", source_effect |-> "WorkFailed", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "ObserveWorkFailed", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None), work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "work_failed_reaches_mob", source_machine |-> "meerkat", source_effect |-> "WorkFailed", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "work_failed_reaches_mob", source_machine |-> "meerkat", effect |-> "WorkFailed", target_machine |-> "mob", target_input |-> "ObserveWorkFailed", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None), work_id |-> packet.payload.work_id], actor |-> "mob_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "RunFailed"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "WorkFailed", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None), generation |-> (IF "value" \in DOMAIN meerkat_active_generation THEN meerkat_active_generation["value"] ELSE None), work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "RunFailed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RunFailed", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RunCancelled(arg_work_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RunCancelled"
       /\ packet.payload.work_id = arg_work_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id = Some(packet.payload.work_id))
       /\ meerkat_phase' = "Attached"
       /\ meerkat_active_work_id' = None
       /\ meerkat_interrupt_pending' = FALSE
       /\ meerkat_shutdown_pending' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "mob", variant |-> "ObserveWorkCancelled", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None), work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "work_cancelled_reaches_mob", source_machine |-> "meerkat", source_effect |-> "WorkCancelled", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "ObserveWorkCancelled", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None), work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "work_cancelled_reaches_mob", source_machine |-> "meerkat", source_effect |-> "WorkCancelled", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "work_cancelled_reaches_mob", source_machine |-> "meerkat", effect |-> "WorkCancelled", target_machine |-> "mob", target_input |-> "ObserveWorkCancelled", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None), work_id |-> packet.payload.work_id], actor |-> "mob_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "RunCancelled"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "WorkCancelled", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None), generation |-> (IF "value" \in DOMAIN meerkat_active_generation THEN meerkat_active_generation["value"] ELSE None), work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "RunCancelled"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RunCancelled", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RecoverFromIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Recover"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeNotice", payload |-> [detail |-> "runtime recovering", kind |-> "recover"], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverFromIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RecoverFromIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RecoverFromAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Recover"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeNotice", payload |-> [detail |-> "runtime recovering", kind |-> "recover"], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverFromAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RecoverFromAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RetireRequestedFromIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Retire"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle" \/ meerkat_phase = "Attached" \/ meerkat_phase = "Running"
       /\ meerkat_phase' = "Retired"
       /\ meerkat_active_work_id' = None
       /\ meerkat_interrupt_pending' = FALSE
       /\ meerkat_shutdown_pending' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "mob", variant |-> "ObserveRuntimeRetired", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_retired_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeRetired", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "ObserveRuntimeRetired", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_retired_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeRetired", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "runtime_retired_reaches_mob", source_machine |-> "meerkat", effect |-> "RuntimeRetired", target_machine |-> "mob", target_input |-> "ObserveRuntimeRetired", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None)], actor |-> "mob_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "RetireRequestedFromIdle"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeRetired", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None), generation |-> (IF "value" \in DOMAIN meerkat_active_generation THEN meerkat_active_generation["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "RetireRequestedFromIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RetireRequestedFromIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_Reset ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Reset"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Initializing" \/ meerkat_phase = "Idle" \/ meerkat_phase = "Attached" \/ meerkat_phase = "Recovering" \/ meerkat_phase = "Retired"
       /\ meerkat_phase' = "Idle"
       /\ meerkat_active_runtime_id' = None
       /\ meerkat_active_fence_token' = None
       /\ meerkat_active_generation' = None
       /\ meerkat_active_work_id' = None
       /\ meerkat_wake_pending' = FALSE
       /\ meerkat_process_pending' = FALSE
       /\ meerkat_drain_running' = FALSE
       /\ meerkat_interrupt_pending' = FALSE
       /\ meerkat_shutdown_pending' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_peer_ingress_configured, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeNotice", payload |-> [detail |-> "runtime reset", kind |-> "reset"], effect_id |-> (model_step_count + 1), source_transition |-> "Reset"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "Reset", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StopRuntimeExecutor ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StopRuntimeExecutor"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Initializing" \/ meerkat_phase = "Idle" \/ meerkat_phase = "Attached" \/ meerkat_phase = "Running" \/ meerkat_phase = "Recovering" \/ meerkat_phase = "Retired"
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_active_work_id' = None
       /\ meerkat_drain_running' = FALSE
       /\ meerkat_interrupt_pending' = FALSE
       /\ meerkat_shutdown_pending' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeNotice", payload |-> [detail |-> "runtime executor stopped", kind |-> "stop"], effect_id |-> (model_step_count + 1), source_transition |-> "StopRuntimeExecutor"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StopRuntimeExecutor", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_Destroy ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Destroy"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Initializing" \/ meerkat_phase = "Idle" \/ meerkat_phase = "Attached" \/ meerkat_phase = "Running" \/ meerkat_phase = "Recovering" \/ meerkat_phase = "Retired" \/ meerkat_phase = "Stopped"
       /\ (meerkat_active_runtime_id # None)
       /\ meerkat_phase' = "Destroyed"
       /\ meerkat_active_work_id' = None
       /\ meerkat_wake_pending' = FALSE
       /\ meerkat_process_pending' = FALSE
       /\ meerkat_drain_running' = FALSE
       /\ meerkat_interrupt_pending' = FALSE
       /\ meerkat_shutdown_pending' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_peer_ingress_configured, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "mob", variant |-> "ObserveRuntimeDestroyed", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_destroyed_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeDestroyed", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "ObserveRuntimeDestroyed", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_destroyed_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeDestroyed", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "runtime_destroyed_reaches_mob", source_machine |-> "meerkat", effect |-> "RuntimeDestroyed", target_machine |-> "mob", target_input |-> "ObserveRuntimeDestroyed", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None)], actor |-> "mob_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "Destroy"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeDestroyed", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None), generation |-> (IF "value" \in DOMAIN meerkat_active_generation THEN meerkat_active_generation["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "Destroy"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "Destroy", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


meerkat_EnsureSessionWithExecutorIdle(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "EnsureSessionWithExecutor"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "EnsureSessionWithExecutorIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "SetSilentIntentsIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ContainsSessionIdle(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ContainsSession"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ContainsSessionIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_SessionHasExecutorIdle(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "SessionHasExecutor"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "SessionHasExecutorIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_SessionHasCommsIdle(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "SessionHasComms"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "SessionHasCommsIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_OpsLifecycleRegistryIdle(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "OpsLifecycleRegistry"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "OpsLifecycleRegistryIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_InputStateIdle(arg_session_id, arg_input_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "InputState"
       /\ packet.payload.session_id = arg_session_id
       /\ packet.payload.input_id = arg_input_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "InputStateIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ListActiveInputsIdle(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ListActiveInputs"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Idle"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ListActiveInputsIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AbortRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "WaitRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AbortAllAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AbortAll"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ meerkat_phase' = "Attached"
       /\ meerkat_drain_running' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ meerkat_drain_running' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AbortAllRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AbortAllRecovering ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AbortAll"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Recovering"
       /\ meerkat_phase' = "Recovering"
       /\ meerkat_drain_running' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AbortAllRecovering", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Recovering"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AbortAllRetired ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AbortAll"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Retired"
       /\ meerkat_phase' = "Retired"
       /\ meerkat_drain_running' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ meerkat_drain_running' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ (meerkat_peer_ingress_configured = TRUE)
       /\ meerkat_phase' = "Attached"
       /\ meerkat_drain_running' = TRUE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ (meerkat_peer_ingress_configured = TRUE)
       /\ meerkat_phase' = "Running"
       /\ meerkat_drain_running' = TRUE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "SpawnDrainTask", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "EnsureDrainRunningRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "EnsureDrainRunningRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_IngestAttached(arg_runtime_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Ingest"
       /\ packet.payload.runtime_id = arg_runtime_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "ResolveAdmission", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "IngestAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "IngestAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_IngestRunning(arg_runtime_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Ingest"
       /\ packet.payload.runtime_id = arg_runtime_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "ResolveAdmission", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "IngestRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "IngestRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "IngressNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PublishEventRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PublishEventRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AcceptWithCompletionAttached(arg_input_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AcceptWithCompletion"
       /\ packet.payload.input_id = arg_input_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "IngressAccepted", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithCompletionAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AcceptWithCompletionAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AcceptWithCompletionRunning(arg_input_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AcceptWithCompletion"
       /\ packet.payload.input_id = arg_input_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "IngressAccepted", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AcceptWithCompletionRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AcceptWithCompletionRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "EnqueueClassifiedEntry", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyPlainEventRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ClassifyPlainEventRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RuntimeStateAttached(arg_runtime_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RuntimeState"
       /\ packet.payload.runtime_id = arg_runtime_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_active_runtime_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RuntimeStateAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RuntimeStateRunning(arg_runtime_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RuntimeState"
       /\ packet.payload.runtime_id = arg_runtime_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_runtime_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RuntimeStateRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_LoadBoundaryReceiptAttached(arg_runtime_id, arg_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "LoadBoundaryReceipt"
       /\ packet.payload.runtime_id = arg_runtime_id
       /\ packet.payload.sequence = arg_sequence
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_active_runtime_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "LoadBoundaryReceiptAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_LoadBoundaryReceiptRunning(arg_runtime_id, arg_sequence) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "LoadBoundaryReceipt"
       /\ packet.payload.runtime_id = arg_runtime_id
       /\ packet.payload.sequence = arg_sequence
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_runtime_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "LoadBoundaryReceiptRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PrepareAttached(arg_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Prepare"
       /\ packet.payload.session_id = arg_session_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "SubmitRunPrimitive", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PrepareAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PrepareAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StartConversationRunAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StartConversationRun"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "SubmitRunPrimitive", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StartImmediateContextAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StartImmediateContextAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CommitRunning(arg_input_id, arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Commit"
       /\ packet.payload.input_id = arg_input_id
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CommitRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_FailRunning(arg_run_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "Fail"
       /\ packet.payload.run_id = arg_run_id
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RecordTerminalOutcome", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FailRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "FailRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AdmitQueuedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AdmitQueued"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "ResolveAdmission", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitQueuedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AdmitQueuedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AdmitConsumedOnAcceptRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AdmitConsumedOnAccept"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "ResolveAdmission", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AdmitConsumedOnAcceptRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AdmitConsumedOnAcceptRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StageDrainSnapshotRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StageDrainSnapshot"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StageDrainSnapshotRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_SupersedeQueuedInputRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "SupersedeQueuedInput"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "SupersedeQueuedInputRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CoalesceQueuedInputsRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "CoalesceQueuedInputs"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CoalesceQueuedInputsRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_SetSilentIntentOverridesRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "SetSilentIntentOverrides"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "SilentIntentApplied", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "SetSilentIntentOverridesRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "SetSilentIntentOverridesRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PrimitiveAppliedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PrimitiveApplied"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "SubmitRunPrimitive", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PrimitiveAppliedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PrimitiveAppliedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_LlmReturnedToolCallsRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "LlmReturnedToolCalls"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "LlmReturnedToolCallsRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_LlmReturnedTerminalRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "LlmReturnedTerminal"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RecordTerminalOutcome", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "LlmReturnedTerminalRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "LlmReturnedTerminalRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RegisterPendingOpsRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RegisterPendingOps"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "SubmitOpEvent", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RegisterPendingOpsRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RegisterPendingOpsRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ToolCallsResolvedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ToolCallsResolved"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "SubmitOpEvent", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ToolCallsResolvedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ToolCallsResolvedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_OpsBarrierSatisfiedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "OpsBarrierSatisfied"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "SubmitOpEvent", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "OpsBarrierSatisfiedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "OpsBarrierSatisfiedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_BoundaryContinueRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "BoundaryContinue"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "BoundaryContinueRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_BoundaryCompleteRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "BoundaryComplete"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RecordBoundarySequence", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BoundaryCompleteRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "BoundaryCompleteRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RecoverableFailureRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RecoverableFailure"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RecordTerminalOutcome", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverableFailureRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RecoverableFailureRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_FatalFailureRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "FatalFailure"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RecordTerminalOutcome", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FatalFailureRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "FatalFailureRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RetryRequestedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RetryRequested"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "SubmitRunPrimitive", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RetryRequestedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RetryRequestedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CancelNowRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "CancelNow"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RequestCancellationAtBoundary", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CancelNowRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CancelNowRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CancellationObservedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "CancellationObserved"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RecordTerminalOutcome", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CancellationObservedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CancellationObservedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AcknowledgeTerminalRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AcknowledgeTerminal"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AcknowledgeTerminalRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_TurnLimitReachedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "TurnLimitReached"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RecordTerminalOutcome", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "TurnLimitReachedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "TurnLimitReachedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_BudgetExhaustedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "BudgetExhausted"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RecordTerminalOutcome", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BudgetExhaustedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "BudgetExhaustedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_TimeBudgetExceededRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "TimeBudgetExceeded"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RecordTerminalOutcome", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "TimeBudgetExceededRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "TimeBudgetExceededRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_EnterExtractionRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "EnterExtraction"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "EnterExtractionRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ExtractionValidationPassedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ExtractionValidationPassed"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ExtractionValidationPassedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ExtractionValidationFailedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ExtractionValidationFailed"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RecordTerminalOutcome", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ExtractionValidationFailedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ExtractionValidationFailedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ExtractionStartRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ExtractionStart"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ExtractionStartRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ForceCancelNoRunRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ForceCancelNoRun"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RequestCancellationAtBoundary", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ForceCancelNoRunRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ForceCancelNoRunRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RegisterOperationRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RegisterOperation"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "SubmitOpEvent", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RegisterOperationRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RegisterOperationRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ProvisioningSucceededRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ProvisioningSucceeded"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "NotifyOpWatcher", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ProvisioningSucceededRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ProvisioningSucceededRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ProvisioningFailedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ProvisioningFailed"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "NotifyOpWatcher", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ProvisioningFailedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ProvisioningFailedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_AbortProvisioningRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "AbortProvisioning"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "NotifyOpWatcher", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AbortProvisioningRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "AbortProvisioningRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_PeerReadyRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "PeerReady"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "ExposeOperationPeer", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PeerReadyRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "PeerReadyRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RegisterWatcherRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RegisterWatcher"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "NotifyOpWatcher", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RegisterWatcherRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RegisterWatcherRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ProgressReportedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ProgressReported"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "NotifyOpWatcher", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ProgressReportedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ProgressReportedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CompleteOperationRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "CompleteOperation"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "CompletionResolved", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteOperationRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CompleteOperationRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_FailOperationRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "FailOperation"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "CompletionResolved", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FailOperationRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "FailOperationRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CancelOperationRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "CancelOperation"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "CompletionResolved", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CancelOperationRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CancelOperationRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RetireRequestedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RetireRequested"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "CheckCompaction", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RetireRequestedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RetireRequestedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RetireCompletedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RetireCompleted"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "CheckCompaction", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RetireCompletedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RetireCompletedRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CollectTerminalRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "CollectTerminal"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "CollectCompletedResult", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CollectTerminalRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CollectTerminalRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_BeginWaitAllRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "BeginWaitAll"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "BeginWaitAllRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_CancelWaitAllRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "CancelWaitAll"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Running"
       /\ (meerkat_active_work_id # None)
       /\ meerkat_phase' = "Running"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "CancelWaitAllRunning", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StageAddAttached ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StageAdd"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached"
       /\ (meerkat_session_id # None)
       /\ meerkat_phase' = "Attached"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ meerkat_active_runtime_id' = None
       /\ meerkat_active_fence_token' = None
       /\ meerkat_active_generation' = None
       /\ meerkat_active_work_id' = None
       /\ meerkat_wake_pending' = FALSE
       /\ meerkat_process_pending' = FALSE
       /\ meerkat_peer_ingress_configured' = FALSE
       /\ meerkat_drain_running' = FALSE
       /\ meerkat_interrupt_pending' = FALSE
       /\ meerkat_shutdown_pending' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ meerkat_active_runtime_id' = None
       /\ meerkat_active_fence_token' = None
       /\ meerkat_active_generation' = None
       /\ meerkat_active_work_id' = None
       /\ meerkat_wake_pending' = FALSE
       /\ meerkat_process_pending' = FALSE
       /\ meerkat_peer_ingress_configured' = FALSE
       /\ meerkat_drain_running' = FALSE
       /\ meerkat_interrupt_pending' = FALSE
       /\ meerkat_shutdown_pending' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "InitiateRecycle", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RecycleFromAttached"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RecycleFromAttached", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_running_has_active_work == ((meerkat_phase # "Running") \/ (meerkat_active_work_id # None))
meerkat_bound_runtime_has_fence == ((meerkat_active_runtime_id = None) \/ (meerkat_active_fence_token # None))
meerkat_destroyed_has_no_active_work == ((meerkat_phase # "Destroyed") \/ (meerkat_active_work_id = None))
meerkat_interrupt_pending_only_while_active == ((meerkat_interrupt_pending = FALSE) \/ (meerkat_phase = "Running"))
meerkat_drain_requires_ingress_context == ((meerkat_drain_running = FALSE) \/ (meerkat_peer_ingress_configured = TRUE))
meerkat_peer_reachability_keys_are_resolved == (\A key \in DOMAIN meerkat_peer_reachability : (key \in meerkat_resolved_peer_keys))
meerkat_peer_last_reason_keys_are_resolved == (\A key \in DOMAIN meerkat_peer_last_reason : (key \in meerkat_resolved_peer_keys))
meerkat_active_visibility_revision_not_ahead_of_staged == (meerkat_active_visibility_revision <= meerkat_staged_visibility_revision)
meerkat_active_requested_names_subset_of_staged == (\A name \in meerkat_active_requested_deferred_names : (name \in meerkat_staged_requested_deferred_names))
meerkat_equal_visibility_revision_means_equal_active_and_staged_state == ((meerkat_active_visibility_revision # meerkat_staged_visibility_revision) \/ ((meerkat_active_filter = meerkat_staged_filter) /\ (meerkat_active_requested_deferred_names = meerkat_staged_requested_deferred_names)))
meerkat_committed_visibility_not_ahead_of_active == (meerkat_committed_visibility_revision <= meerkat_active_visibility_revision)

mob_Start ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Start"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating" \/ mob_phase = "Stopped"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "Start", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_Spawn(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Spawn"
       /\ packet.payload.agent_identity = arg_agent_identity
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.generation = arg_generation
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating" \/ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_active_identity' = Some(packet.payload.agent_identity)
       /\ mob_active_runtime_id' = Some(packet.payload.agent_runtime_id)
       /\ mob_active_fence_token' = Some(packet.payload.fence_token)
       /\ mob_current_generation' = Some(packet.payload.generation)
       /\ mob_inflight_work_id' = None
       /\ mob_active_member_count' = 1
       /\ mob_active_run_count' = 0
       /\ mob_pending_spawn_count' = 0
       /\ mob_retiring_member_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "PrepareBindings", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None), generation |-> (IF "value" \in DOMAIN Some(packet.payload.generation) THEN Some(packet.payload.generation)["value"] ELSE None)], source_kind |-> "route", source_route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeBinding", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "PrepareBindings", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None), generation |-> (IF "value" \in DOMAIN Some(packet.payload.generation) THEN Some(packet.payload.generation)["value"] ELSE None)], source_kind |-> "route", source_route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeBinding", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", effect |-> "RequestRuntimeBinding", target_machine |-> "meerkat", target_input |-> "PrepareBindings", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None), generation |-> (IF "value" \in DOMAIN Some(packet.payload.generation) THEN Some(packet.payload.generation)["value"] ELSE None)], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "Spawn"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestRuntimeBinding", payload |-> [agent_identity |-> (IF "value" \in DOMAIN Some(packet.payload.agent_identity) THEN Some(packet.payload.agent_identity)["value"] ELSE None), agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None), generation |-> (IF "value" \in DOMAIN Some(packet.payload.generation) THEN Some(packet.payload.generation)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "Spawn"], [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [agent_identity |-> (IF "value" \in DOMAIN Some(packet.payload.agent_identity) THEN Some(packet.payload.agent_identity)["value"] ELSE None), kind |-> "spawned"], effect_id |-> (model_step_count + 1), source_transition |-> "Spawn"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "Spawn", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ObserveRuntimeReady", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_SubmitWork(arg_agent_runtime_id, arg_fence_token, arg_work_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubmitWork"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.work_id = arg_work_id
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_active_runtime_id # None)
       /\ mob_phase' = "Running"
       /\ mob_inflight_work_id' = Some(packet.payload.work_id)
       /\ mob_active_run_count' = (mob_active_run_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "SubmitMobWork", payload |-> [agent_runtime_id |-> packet.payload.agent_runtime_id, fence_token |-> packet.payload.fence_token, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "member_work_reaches_meerkat", source_machine |-> "mob", source_effect |-> "SubmitMemberWork", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "SubmitMobWork", payload |-> [agent_runtime_id |-> packet.payload.agent_runtime_id, fence_token |-> packet.payload.fence_token, work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "member_work_reaches_meerkat", source_machine |-> "mob", source_effect |-> "SubmitMemberWork", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "member_work_reaches_meerkat", source_machine |-> "mob", effect |-> "SubmitMemberWork", target_machine |-> "meerkat", target_input |-> "SubmitMobWork", payload |-> [agent_runtime_id |-> packet.payload.agent_runtime_id, fence_token |-> packet.payload.fence_token, work_id |-> packet.payload.work_id], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "SubmitWork"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "SubmitMemberWork", payload |-> [agent_runtime_id |-> packet.payload.agent_runtime_id, fence_token |-> packet.payload.fence_token, work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "SubmitWork"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SubmitWork", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_ObserveWorkCompleted(arg_agent_runtime_id, arg_fence_token, arg_work_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ObserveWorkCompleted"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.work_id = arg_work_id
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ObserveWorkCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_ObserveWorkFailed(arg_agent_runtime_id, arg_fence_token, arg_work_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ObserveWorkFailed"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.work_id = arg_work_id
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ObserveWorkFailed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_ObserveWorkCancelled(arg_agent_runtime_id, arg_fence_token, arg_work_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ObserveWorkCancelled"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.work_id = arg_work_id
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ObserveWorkCancelled", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RetireMember(arg_agent_runtime_id, arg_fence_token) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RetireMember"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "Retire", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeRetire", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "Retire", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeRetire", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", effect |-> "RequestRuntimeRetire", target_machine |-> "meerkat", target_input |-> "Retire", payload |-> [tag |-> "unit"], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "RetireMember"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestRuntimeRetire", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN mob_active_runtime_id THEN mob_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN mob_active_fence_token THEN mob_active_fence_token["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "RetireMember"] }
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
       /\ mob_phase' = "Stopped"
       /\ mob_active_runtime_id' = None
       /\ mob_active_fence_token' = None
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [agent_identity |-> (IF "value" \in DOMAIN mob_active_identity THEN mob_active_identity["value"] ELSE None), kind |-> "retired"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveRuntimeRetired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ObserveRuntimeRetired", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_ResetMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ResetMember"
       /\ packet.payload.agent_identity = arg_agent_identity
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.generation = arg_generation
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running" \/ mob_phase = "Stopped"
       /\ mob_phase' = "Running"
       /\ mob_active_identity' = Some(packet.payload.agent_identity)
       /\ mob_active_runtime_id' = Some(packet.payload.agent_runtime_id)
       /\ mob_active_fence_token' = Some(packet.payload.fence_token)
       /\ mob_current_generation' = Some(packet.payload.generation)
       /\ mob_inflight_work_id' = None
       /\ mob_active_member_count' = 1
       /\ mob_active_run_count' = 0
       /\ mob_pending_spawn_count' = 0
       /\ mob_retiring_member_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "PrepareBindings", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None), generation |-> (IF "value" \in DOMAIN Some(packet.payload.generation) THEN Some(packet.payload.generation)["value"] ELSE None)], source_kind |-> "route", source_route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeBinding", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "PrepareBindings", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None), generation |-> (IF "value" \in DOMAIN Some(packet.payload.generation) THEN Some(packet.payload.generation)["value"] ELSE None)], source_kind |-> "route", source_route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeBinding", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", effect |-> "RequestRuntimeBinding", target_machine |-> "meerkat", target_input |-> "PrepareBindings", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None), generation |-> (IF "value" \in DOMAIN Some(packet.payload.generation) THEN Some(packet.payload.generation)["value"] ELSE None)], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "ResetMember"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestRuntimeBinding", payload |-> [agent_identity |-> (IF "value" \in DOMAIN Some(packet.payload.agent_identity) THEN Some(packet.payload.agent_identity)["value"] ELSE None), agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None), generation |-> (IF "value" \in DOMAIN Some(packet.payload.generation) THEN Some(packet.payload.generation)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "ResetMember"], [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [agent_identity |-> (IF "value" \in DOMAIN Some(packet.payload.agent_identity) THEN Some(packet.payload.agent_identity)["value"] ELSE None), kind |-> "reset"], effect_id |-> (model_step_count + 1), source_transition |-> "ResetMember"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ResetMember", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RespawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RespawnMember"
       /\ packet.payload.agent_identity = arg_agent_identity
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.generation = arg_generation
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating" \/ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_active_identity' = Some(packet.payload.agent_identity)
       /\ mob_active_runtime_id' = Some(packet.payload.agent_runtime_id)
       /\ mob_active_fence_token' = Some(packet.payload.fence_token)
       /\ mob_current_generation' = Some(packet.payload.generation)
       /\ mob_inflight_work_id' = None
       /\ mob_active_member_count' = 1
       /\ mob_active_run_count' = 0
       /\ mob_pending_spawn_count' = 0
       /\ mob_retiring_member_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "PrepareBindings", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None), generation |-> (IF "value" \in DOMAIN Some(packet.payload.generation) THEN Some(packet.payload.generation)["value"] ELSE None)], source_kind |-> "route", source_route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeBinding", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "PrepareBindings", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None), generation |-> (IF "value" \in DOMAIN Some(packet.payload.generation) THEN Some(packet.payload.generation)["value"] ELSE None)], source_kind |-> "route", source_route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeBinding", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", effect |-> "RequestRuntimeBinding", target_machine |-> "meerkat", target_input |-> "PrepareBindings", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None), generation |-> (IF "value" \in DOMAIN Some(packet.payload.generation) THEN Some(packet.payload.generation)["value"] ELSE None)], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "RespawnMember"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestRuntimeBinding", payload |-> [agent_identity |-> (IF "value" \in DOMAIN Some(packet.payload.agent_identity) THEN Some(packet.payload.agent_identity)["value"] ELSE None), agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None), generation |-> (IF "value" \in DOMAIN Some(packet.payload.generation) THEN Some(packet.payload.generation)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "RespawnMember"], [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [agent_identity |-> (IF "value" \in DOMAIN Some(packet.payload.agent_identity) THEN Some(packet.payload.agent_identity)["value"] ELSE None), kind |-> "respawned"], effect_id |-> (model_step_count + 1), source_transition |-> "RespawnMember"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RespawnMember", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_MarkCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "MarkCompleted"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running" \/ mob_phase = "Stopped"
       /\ (mob_inflight_work_id = None)
       /\ mob_phase' = "Completed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [agent_identity |-> (IF "value" \in DOMAIN mob_active_identity THEN mob_active_identity["value"] ELSE None), kind |-> "completed"], effect_id |-> (model_step_count + 1), source_transition |-> "MarkCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "MarkCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_DestroyMob ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "DestroyMob"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating" \/ mob_phase = "Running" \/ mob_phase = "Stopped" \/ mob_phase = "Completed"
       /\ mob_phase' = "Destroyed"
       /\ mob_active_identity' = None
       /\ mob_active_runtime_id' = None
       /\ mob_active_fence_token' = None
       /\ mob_current_generation' = None
       /\ mob_inflight_work_id' = None
       /\ mob_active_member_count' = 0
       /\ mob_active_run_count' = 0
       /\ mob_pending_spawn_count' = 0
       /\ mob_retiring_member_count' = 0
       /\ mob_wiring_edge_count' = 0
       /\ mob_task_count' = 0
       /\ mob_event_subscription_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_coordinator_bound' = FALSE
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "Destroy", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "destroy_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeDestroy", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "Destroy", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "destroy_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeDestroy", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "destroy_request_reaches_meerkat", source_machine |-> "mob", effect |-> "RequestRuntimeDestroy", target_machine |-> "meerkat", target_input |-> "Destroy", payload |-> [tag |-> "unit"], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "DestroyMob"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestRuntimeDestroy", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN None THEN None["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN None THEN None["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "DestroyMob"] }
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
       /\ mob_phase' = "Destroyed"
       /\ mob_active_identity' = None
       /\ mob_active_runtime_id' = None
       /\ mob_active_fence_token' = None
       /\ mob_current_generation' = None
       /\ mob_inflight_work_id' = None
       /\ mob_active_member_count' = 0
       /\ mob_active_run_count' = 0
       /\ mob_pending_spawn_count' = 0
       /\ mob_retiring_member_count' = 0
       /\ mob_wiring_edge_count' = 0
       /\ mob_task_count' = 0
       /\ mob_event_subscription_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_coordinator_bound' = FALSE
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [agent_identity |-> (IF "value" \in DOMAIN None THEN None["value"] ELSE None), kind |-> "destroyed"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveRuntimeDestroyed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ObserveRuntimeDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_FlowStatusCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "FlowStatus"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "FlowStatusCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_FlowStatusRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "FlowStatus"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "FlowStatusRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_FlowStatusStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "FlowStatus"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "FlowStatusStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_FlowStatusCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "FlowStatus"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Completed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "FlowStatusCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_FlowStatusDestroyed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "FlowStatus"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Destroyed"
       /\ mob_phase' = "Destroyed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "FlowStatusDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_McpServerStatesCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "McpServerStates"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "McpServerStatesCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_McpServerStatesRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "McpServerStates"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "McpServerStatesRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_McpServerStatesStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "McpServerStates"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "McpServerStatesStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_McpServerStatesCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "McpServerStates"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Completed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "McpServerStatesCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_McpServerStatesDestroyed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "McpServerStates"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Destroyed"
       /\ mob_phase' = "Destroyed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "McpServerStatesDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_RosterSnapshotCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RosterSnapshot"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RosterSnapshotCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_RosterSnapshotRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RosterSnapshot"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RosterSnapshotRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RosterSnapshotStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RosterSnapshot"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RosterSnapshotStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_RosterSnapshotCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RosterSnapshot"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Completed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RosterSnapshotCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_RosterSnapshotDestroyed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RosterSnapshot"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Destroyed"
       /\ mob_phase' = "Destroyed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RosterSnapshotDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_ListMembersCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ListMembers"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ListMembersCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_ListMembersRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ListMembers"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ListMembersRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_ListMembersStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ListMembers"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ListMembersStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_ListMembersCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ListMembers"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Completed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ListMembersCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_ListMembersDestroyed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ListMembers"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Destroyed"
       /\ mob_phase' = "Destroyed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ListMembersDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_ListMembersIncludingRetiringCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ListMembersIncludingRetiring"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ListMembersIncludingRetiringCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_ListMembersIncludingRetiringRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ListMembersIncludingRetiring"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ListMembersIncludingRetiringRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_ListMembersIncludingRetiringStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ListMembersIncludingRetiring"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ListMembersIncludingRetiringStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_ListMembersIncludingRetiringCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ListMembersIncludingRetiring"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Completed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ListMembersIncludingRetiringCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_ListMembersIncludingRetiringDestroyed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ListMembersIncludingRetiring"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Destroyed"
       /\ mob_phase' = "Destroyed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ListMembersIncludingRetiringDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_ListAllMembersCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ListAllMembers"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ListAllMembersCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_ListAllMembersRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ListAllMembers"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ListAllMembersRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_ListAllMembersStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ListAllMembers"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ListAllMembersStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_ListAllMembersCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ListAllMembers"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Completed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ListAllMembersCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_ListAllMembersDestroyed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ListAllMembers"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Destroyed"
       /\ mob_phase' = "Destroyed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ListAllMembersDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_MemberStatusCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "MemberStatus"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "MemberStatusCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_MemberStatusRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "MemberStatus"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "MemberStatusRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_MemberStatusStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "MemberStatus"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "MemberStatusStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_MemberStatusCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "MemberStatus"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Completed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "MemberStatusCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_MemberStatusDestroyed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "MemberStatus"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Destroyed"
       /\ mob_phase' = "Destroyed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "MemberStatusDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_TaskListCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TaskList"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TaskListCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_TaskListRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TaskList"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TaskListRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_TaskListStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TaskList"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TaskListStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_TaskListCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TaskList"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Completed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TaskListCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_TaskListDestroyed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TaskList"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Destroyed"
       /\ mob_phase' = "Destroyed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TaskListDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_TaskGetCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TaskGet"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TaskGetCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_TaskGetRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TaskGet"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TaskGetRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_TaskGetStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TaskGet"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TaskGetStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_TaskGetCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TaskGet"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Completed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TaskGetCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_TaskGetDestroyed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TaskGet"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Destroyed"
       /\ mob_phase' = "Destroyed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TaskGetDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_PollEventsCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "PollEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "PollEventsCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_PollEventsRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "PollEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "PollEventsRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_PollEventsStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "PollEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "PollEventsStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_PollEventsCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "PollEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Completed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "PollEventsCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_PollEventsDestroyed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "PollEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Destroyed"
       /\ mob_phase' = "Destroyed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "PollEventsDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_ReplayAllEventsCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ReplayAllEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ReplayAllEventsCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_ReplayAllEventsRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ReplayAllEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ReplayAllEventsRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_ReplayAllEventsStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ReplayAllEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ReplayAllEventsStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_ReplayAllEventsCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ReplayAllEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Completed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ReplayAllEventsCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_ReplayAllEventsDestroyed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ReplayAllEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Destroyed"
       /\ mob_phase' = "Destroyed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ReplayAllEventsDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_RecordOperatorActionProvenanceCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RecordOperatorActionProvenance"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RecordOperatorActionProvenanceCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_RecordOperatorActionProvenanceRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RecordOperatorActionProvenance"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RecordOperatorActionProvenanceDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_GetMemberCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "GetMember"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "GetMemberCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_GetMemberRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "GetMember"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "GetMemberRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_GetMemberStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "GetMember"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "GetMemberStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_GetMemberCompleted ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "GetMember"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Completed"
       /\ mob_phase' = "Completed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "GetMemberCompleted", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


mob_GetMemberDestroyed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "GetMember"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Destroyed"
       /\ mob_phase' = "Destroyed"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "GetMemberDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_SetSpawnPolicyCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SetSpawnPolicy"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SetSpawnPolicyCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_SetSpawnPolicyRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SetSpawnPolicy"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ mob_phase' = "Stopped"
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_pending_spawn_count' = 0
       /\ mob_retiring_member_count' = 0
       /\ mob_wiring_edge_count' = 0
       /\ mob_task_count' = 0
       /\ mob_event_subscription_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_coordinator_bound' = FALSE
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ResetToRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ResetToRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_WireCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Wire"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ mob_wiring_edge_count' = (mob_wiring_edge_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "WireCreating"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "WireCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_WireRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Wire"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_wiring_edge_count' = (mob_wiring_edge_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "WireRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "WireRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_ExternalTurnCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ExternalTurn"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitProgressNote", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ExternalTurnCreating"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ExternalTurnCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_ExternalTurnRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ExternalTurn"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitProgressNote", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ExternalTurnRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ExternalTurnRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_InternalTurnCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "InternalTurn"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitProgressNote", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "InternalTurnCreating"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "InternalTurnCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_InternalTurnRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "InternalTurn"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitProgressNote", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "InternalTurnRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "InternalTurnRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_TaskCreateCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TaskCreate"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ mob_task_count' = (mob_task_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitTaskNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "TaskCreateCreating"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TaskCreateCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_TaskCreateRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TaskCreate"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_task_count' = (mob_task_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitTaskNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "TaskCreateRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TaskCreateRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_TaskUpdateCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TaskUpdate"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitTaskNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "TaskUpdateCreating"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TaskUpdateCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_TaskUpdateRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TaskUpdate"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitTaskNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "TaskUpdateRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TaskUpdateRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_ForceCancelCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ForceCancel"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "FlowTerminalized", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ForceCancelCreating"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ForceCancelCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_ForceCancelRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ForceCancel"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "FlowTerminalized", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ForceCancelRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ForceCancelRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_SubscribeAgentEventsCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubscribeAgentEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ mob_event_subscription_count' = (mob_event_subscription_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SubscribeAgentEventsCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_SubscribeAgentEventsRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubscribeAgentEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_event_subscription_count' = (mob_event_subscription_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ mob_phase' = "Stopped"
       /\ mob_event_subscription_count' = (mob_event_subscription_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ mob_phase' = "Completed"
       /\ mob_event_subscription_count' = (mob_event_subscription_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ mob_phase' = "Destroyed"
       /\ mob_event_subscription_count' = (mob_event_subscription_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SubscribeAgentEventsDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_SubscribeAllAgentEventsCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubscribeAllAgentEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ mob_event_subscription_count' = (mob_event_subscription_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SubscribeAllAgentEventsCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_SubscribeAllAgentEventsRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubscribeAllAgentEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_event_subscription_count' = (mob_event_subscription_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ mob_event_subscription_count' = (mob_event_subscription_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ mob_event_subscription_count' = (mob_event_subscription_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ mob_event_subscription_count' = (mob_event_subscription_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SubscribeAllAgentEventsDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_SubscribeMobEventsCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubscribeMobEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ mob_event_subscription_count' = (mob_event_subscription_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SubscribeMobEventsCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_SubscribeMobEventsRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SubscribeMobEvents"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_event_subscription_count' = (mob_event_subscription_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ mob_event_subscription_count' = (mob_event_subscription_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ mob_event_subscription_count' = (mob_event_subscription_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ mob_event_subscription_count' = (mob_event_subscription_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ShutdownRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ShutdownRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_ShutdownCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Shutdown"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ShutdownCreating"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ShutdownCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_ShutdownStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Shutdown"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StopOrchestratorRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "StopOrchestratorRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_ResumeOrchestratorRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ResumeOrchestrator"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_coordinator_bound' = TRUE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ResumeOrchestratorRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ResumeOrchestratorRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_DestroyOrchestratorRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "DestroyOrchestrator"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_coordinator_bound' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "DestroyOrchestratorRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "DestroyOrchestratorRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_ForceCancelMemberRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ForceCancelMember"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "AdmitPeerInput", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "PeerInputAdmittedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "PeerInputAdmittedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RuntimeWorkAdmittedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RuntimeWorkAdmitted"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "AdmitStepWork", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RuntimeWorkAdmittedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RuntimeWorkAdmittedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_KickoffFailedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "KickoffFailed"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitMemberTerminalNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "KickoffFailedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "KickoffFailedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_KickoffCancelledRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "KickoffCancelled"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitMemberTerminalNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "KickoffCancelledRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "KickoffCancelledRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_KickoffForceCancelledRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "KickoffForceCancelled"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitMemberTerminalNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "KickoffForceCancelledRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "KickoffForceCancelledRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RuntimeRunSubmittedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RuntimeRunSubmitted"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RuntimeRunSubmittedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RuntimeRunSubmittedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RuntimeRunCompletedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RuntimeRunCompleted"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RuntimeRunCompletedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RuntimeRunCompletedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RuntimeRunFailedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RuntimeRunFailed"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RuntimeRunFailedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RuntimeRunFailedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RuntimeRunCancelledRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RuntimeRunCancelled"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RuntimeRunCancelledRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RuntimeRunCancelledRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RuntimeStopRequestedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RuntimeStopRequested"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RuntimeStopRequestedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RuntimeStopRequestedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_DispatchStepRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "DispatchStep"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "AdmitStepWork", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "DispatchStepRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "DispatchStepRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_CompleteStepRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "CompleteStep"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitStepNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteStepRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "CompleteStepRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RecordStepOutputRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RecordStepOutput"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "PersistStepOutput", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RecordStepOutputRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RecordStepOutputRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_ConditionPassedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ConditionPassed"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitStepNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ConditionPassedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ConditionPassedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_ConditionRejectedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ConditionRejected"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitStepNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ConditionRejectedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ConditionRejectedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_FailStepRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "FailStep"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitStepNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FailStepRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "FailStepRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_SkipStepRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SkipStep"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitStepNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "SkipStepRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SkipStepRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_ProjectFrameStepStatusRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "ProjectFrameStepStatus"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitStepNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ProjectFrameStepStatusRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ProjectFrameStepStatusRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_CancelStepRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "CancelStep"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitStepNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CancelStepRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "CancelStepRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RegisterTargetsRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RegisterTargets"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RegisterTargetsRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RegisterTargetsRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RecordTargetSuccessRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RecordTargetSuccess"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "ProjectTargetSuccess", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RecordTargetSuccessRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RecordTargetSuccessRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RecordTargetTerminalFailureRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RecordTargetTerminalFailure"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "ProjectTargetFailure", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RecordTargetTerminalFailureRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RecordTargetTerminalFailureRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RecordTargetCanceledRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RecordTargetCanceled"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "ProjectTargetCanceled", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RecordTargetCanceledRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RecordTargetCanceledRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RecordTargetFailureRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RecordTargetFailure"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "ProjectTargetFailure", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RecordTargetFailureRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RecordTargetFailureRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_NodeExecutionReleasedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "NodeExecutionReleased"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NodeExecutionReleased", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "NodeExecutionReleasedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "NodeExecutionReleasedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_TerminalizeCompletedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TerminalizeCompleted"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RootFrameCompleted", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "TerminalizeCompletedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TerminalizeCompletedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_TerminalizeFailedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TerminalizeFailed"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RootFrameFailed", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "TerminalizeFailedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TerminalizeFailedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_TerminalizeCanceledRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "TerminalizeCanceled"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RootFrameCanceled", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "TerminalizeCanceledRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "TerminalizeCanceledRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_CompleteNodeRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "CompleteNode"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitStepNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteNodeRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "CompleteNodeRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RecordNodeOutputRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RecordNodeOutput"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "PersistStepOutput", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RecordNodeOutputRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RecordNodeOutputRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_FailNodeRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "FailNode"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitStepNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FailNodeRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "FailNodeRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_SkipNodeRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SkipNode"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitStepNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "SkipNodeRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SkipNodeRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_CancelNodeRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "CancelNode"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitStepNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CancelNodeRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "CancelNodeRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_UntilConditionMetRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "UntilConditionMet"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EvaluateUntilCondition", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "UntilConditionMetRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "UntilConditionMetRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_BeginCleanupRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "BeginCleanup"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BeginCleanupRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "BeginCleanupRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_FinishCleanupRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "FinishCleanup"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FinishCleanupRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "FinishCleanupRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_KickoffStartedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "KickoffStarted"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_active_member_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_kickoff_pending' = TRUE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "AdmitKickoffTurn", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "KickoffStartedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "KickoffStartedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_KickoffCallbackPendingRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "KickoffCallbackPending"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_active_member_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_kickoff_pending' = TRUE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "KickoffCallbackPendingRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RunFlowRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RunFlow"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_active_member_count > 0)
       /\ (mob_active_runtime_id # None)
       /\ mob_phase' = "Running"
       /\ mob_active_run_count' = (mob_active_run_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ (mob_active_member_count > 0)
       /\ (mob_active_runtime_id # None)
       /\ mob_phase' = "Running"
       /\ mob_active_run_count' = (mob_active_run_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ (mob_active_member_count > 0)
       /\ (mob_active_runtime_id # None)
       /\ mob_phase' = "Running"
       /\ mob_active_run_count' = (mob_active_run_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ (mob_active_member_count > 0)
       /\ (mob_active_runtime_id # None)
       /\ mob_phase' = "Running"
       /\ mob_active_run_count' = (mob_active_run_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StartRunRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "StartRunRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RegisterReadyFrameRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RegisterReadyFrame"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_active_run_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_active_frame_count' = (mob_active_frame_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RegisterReadyFrameRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_UnwireCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Unwire"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ (mob_wiring_edge_count > 0)
       /\ mob_phase' = "Creating"
       /\ mob_wiring_edge_count' = (mob_wiring_edge_count) - 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "UnwireCreating"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "UnwireCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_UnwireRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Unwire"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_wiring_edge_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_wiring_edge_count' = (mob_wiring_edge_count) - 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "NotifyCoordinator", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "UnwireRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "UnwireRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RegisterPendingBodyFrameRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RegisterPendingBodyFrame"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_active_run_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_active_frame_count' = (mob_active_frame_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestBodyFrameStart", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RegisterPendingBodyFrameRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RegisterPendingBodyFrameRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_CompleteFlowRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "CompleteFlow"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_active_run_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "FlowTerminalized", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteFlowRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "CompleteFlowRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_StartRootFrameRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "StartRootFrame"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_active_run_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_active_frame_count' = (mob_active_frame_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "StartRootFrameRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_StartBodyFrameRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "StartBodyFrame"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_active_run_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_active_frame_count' = (mob_active_frame_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestBodyFrameStart", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StartBodyFrameRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "StartBodyFrameRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_FrameTerminatedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "FrameTerminated"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_active_frame_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_active_frame_count' = (mob_active_frame_count) - 1
       /\ mob_active_loop_count' = 0
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "FrameTerminatedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_StartLoopRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "StartLoop"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_active_run_count > 0)
       /\ (mob_active_frame_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_active_loop_count' = (mob_active_loop_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "StartLoopNode", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "StartLoopRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "StartLoopRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_BodyFrameStartedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "BodyFrameStarted"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_active_run_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_active_frame_count' = (mob_active_frame_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestBodyFrameStart", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BodyFrameStartedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "BodyFrameStartedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_BodyFrameCompletedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "BodyFrameCompleted"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_active_frame_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_active_frame_count' = (mob_active_frame_count) - 1
       /\ mob_active_loop_count' = 0
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "BodyFrameCompleted", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BodyFrameCompletedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "BodyFrameCompletedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_BodyFrameFailedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "BodyFrameFailed"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_active_frame_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_active_frame_count' = (mob_active_frame_count) - 1
       /\ mob_active_loop_count' = 0
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "BodyFrameFailed", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BodyFrameFailedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "BodyFrameFailedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_BodyFrameCanceledRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "BodyFrameCanceled"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_active_frame_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_active_frame_count' = (mob_active_frame_count) - 1
       /\ mob_active_loop_count' = 0
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "BodyFrameCanceled", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "BodyFrameCanceledRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "BodyFrameCanceledRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_UntilConditionFailedRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "UntilConditionFailed"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_active_loop_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_active_loop_count' = (mob_active_loop_count) - 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "LoopCompleted", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "UntilConditionFailedRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "UntilConditionFailedRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_CancelLoopRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "CancelLoop"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_active_loop_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_active_loop_count' = (mob_active_loop_count) - 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "LoopCanceled", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CancelLoopRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "CancelLoopRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_FinishRunRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "FinishRun"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_active_run_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitRunLifecycleNotice", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "FinishRunRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "FinishRunRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RetireCreating(arg_agent_runtime_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Retire"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ (mob_active_member_count > 0)
       /\ (mob_active_member_count > mob_retiring_member_count)
       /\ mob_phase' = "Creating"
       /\ mob_retiring_member_count' = (mob_retiring_member_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "Retire", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeRetire", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "Retire", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeRetire", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", effect |-> "RequestRuntimeRetire", target_machine |-> "meerkat", target_input |-> "Retire", payload |-> [tag |-> "unit"], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "RetireCreating"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestRuntimeRetire", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN mob_active_runtime_id THEN mob_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN mob_active_fence_token THEN mob_active_fence_token["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "RetireCreating"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RetireCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_RetireRunning(arg_agent_runtime_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Retire"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_active_member_count > 0)
       /\ (mob_active_member_count > mob_retiring_member_count)
       /\ mob_phase' = "Running"
       /\ mob_retiring_member_count' = (mob_retiring_member_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "Retire", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeRetire", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "Retire", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeRetire", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", effect |-> "RequestRuntimeRetire", target_machine |-> "meerkat", target_input |-> "Retire", payload |-> [tag |-> "unit"], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "RetireRunning"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestRuntimeRetire", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN mob_active_runtime_id THEN mob_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN mob_active_fence_token THEN mob_active_fence_token["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "RetireRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RetireRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RetireStopped(arg_agent_runtime_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Retire"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ (mob_active_member_count > 0)
       /\ (mob_active_member_count > mob_retiring_member_count)
       /\ mob_phase' = "Stopped"
       /\ mob_retiring_member_count' = (mob_retiring_member_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "Retire", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeRetire", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "Retire", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeRetire", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", effect |-> "RequestRuntimeRetire", target_machine |-> "meerkat", target_input |-> "Retire", payload |-> [tag |-> "unit"], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "RetireStopped"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestRuntimeRetire", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN mob_active_runtime_id THEN mob_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN mob_active_fence_token THEN mob_active_fence_token["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "RetireStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RetireStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_RetireAllCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RetireAll"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ mob_retiring_member_count' = mob_active_member_count
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [agent_identity |-> (IF "value" \in DOMAIN mob_active_identity THEN mob_active_identity["value"] ELSE None), kind |-> "retiring"], effect_id |-> (model_step_count + 1), source_transition |-> "RetireAllCreating"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RetireAllCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_RetireAllRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RetireAll"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_retiring_member_count' = mob_active_member_count
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [agent_identity |-> (IF "value" \in DOMAIN mob_active_identity THEN mob_active_identity["value"] ELSE None), kind |-> "retiring"], effect_id |-> (model_step_count + 1), source_transition |-> "RetireAllRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RetireAllRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_RetireAllStopped ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "RetireAll"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Stopped"
       /\ mob_phase' = "Stopped"
       /\ mob_retiring_member_count' = mob_active_member_count
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [agent_identity |-> (IF "value" \in DOMAIN mob_active_identity THEN mob_active_identity["value"] ELSE None), kind |-> "retiring"], effect_id |-> (model_step_count + 1), source_transition |-> "RetireAllStopped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RetireAllStopped", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


mob_CompleteSpawnRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "CompleteSpawn"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ (mob_pending_spawn_count > 0)
       /\ mob_phase' = "Running"
       /\ mob_active_member_count' = (mob_active_member_count) + 1
       /\ mob_pending_spawn_count' = (mob_pending_spawn_count) - 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_run_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [agent_identity |-> (IF "value" \in DOMAIN mob_active_identity THEN mob_active_identity["value"] ELSE None), kind |-> "spawned"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteSpawnRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "CompleteSpawnRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_DestroyFromAny ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Destroy"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating" \/ mob_phase = "Running" \/ mob_phase = "Stopped" \/ mob_phase = "Completed"
       /\ mob_phase' = "Destroyed"
       /\ mob_active_identity' = None
       /\ mob_active_runtime_id' = None
       /\ mob_active_fence_token' = None
       /\ mob_current_generation' = None
       /\ mob_inflight_work_id' = None
       /\ mob_active_member_count' = 0
       /\ mob_active_run_count' = 0
       /\ mob_pending_spawn_count' = 0
       /\ mob_retiring_member_count' = 0
       /\ mob_wiring_edge_count' = 0
       /\ mob_task_count' = 0
       /\ mob_event_subscription_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_coordinator_bound' = FALSE
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [agent_identity |-> (IF "value" \in DOMAIN None THEN None["value"] ELSE None), kind |-> "destroyed"], effect_id |-> (model_step_count + 1), source_transition |-> "DestroyFromAny"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "DestroyFromAny", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_RespawnCreating(arg_agent_runtime_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Respawn"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ mob_pending_spawn_count' = (mob_pending_spawn_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "ExposePendingSpawn", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RespawnCreating"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RespawnCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_RespawnRunning(arg_agent_runtime_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "Respawn"
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_pending_spawn_count' = (mob_pending_spawn_count) + 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "ExposePendingSpawn", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RespawnRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "RespawnRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_CancelWorkRunning(arg_work_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "CancelWork"
       /\ packet.payload.work_id = arg_work_id
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "FlowTerminalized", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CancelWorkRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "CancelWorkRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_CancelAllWorkCreating ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "CancelAllWork"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating"
       /\ mob_phase' = "Creating"
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "FlowTerminalized", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CancelAllWorkCreating"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "CancelAllWorkCreating", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Creating"]}
       /\ model_step_count' = model_step_count + 1


mob_CancelAllWorkRunning ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "CancelAllWork"
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Running"
       /\ mob_phase' = "Running"
       /\ mob_inflight_work_id' = None
       /\ mob_active_run_count' = 0
       /\ mob_active_frame_count' = 0
       /\ mob_active_loop_count' = 0
       /\ mob_kickoff_pending' = FALSE
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_coordinator_bound, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "FlowTerminalized", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CancelAllWorkRunning"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "CancelAllWorkRunning", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_active_work_requires_runtime == ((mob_inflight_work_id = None) \/ (mob_active_runtime_id # None))
mob_destroyed_has_no_active_runtime == ((mob_phase # "Destroyed") \/ (mob_active_runtime_id = None))
mob_active_runtime_has_identity == ((mob_active_runtime_id = None) \/ (mob_active_identity # None))
mob_active_frames_require_runs == ((mob_active_frame_count = 0) \/ (mob_active_run_count > 0))
mob_active_loops_require_frames == ((mob_active_loop_count = 0) \/ (mob_active_frame_count > 0))
mob_retiring_members_do_not_exceed_active_members == (mob_retiring_member_count <= mob_active_member_count)
mob_kickoff_pending_requires_members == ((mob_kickoff_pending = FALSE) \/ (mob_active_member_count > 0))

Inject_spawn_member(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation) ==
    /\ ~([machine |-> "mob", variant |-> "Spawn", payload |-> [agent_identity |-> arg_agent_identity, agent_runtime_id |-> arg_agent_runtime_id, fence_token |-> arg_fence_token, generation |-> arg_generation], source_kind |-> "entry", source_route |-> "spawn_member", source_machine |-> "external_entry", source_effect |-> "Spawn", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "mob", variant |-> "Spawn", payload |-> [agent_identity |-> arg_agent_identity, agent_runtime_id |-> arg_agent_runtime_id, fence_token |-> arg_fence_token, generation |-> arg_generation], source_kind |-> "entry", source_route |-> "spawn_member", source_machine |-> "external_entry", source_effect |-> "Spawn", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "Spawn", payload |-> [agent_identity |-> arg_agent_identity, agent_runtime_id |-> arg_agent_runtime_id, fence_token |-> arg_fence_token, generation |-> arg_generation], source_kind |-> "entry", source_route |-> "spawn_member", source_machine |-> "external_entry", source_effect |-> "Spawn", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_submit_work(arg_agent_runtime_id, arg_fence_token, arg_work_id) ==
    /\ ~([machine |-> "mob", variant |-> "SubmitWork", payload |-> [agent_runtime_id |-> arg_agent_runtime_id, fence_token |-> arg_fence_token, work_id |-> arg_work_id], source_kind |-> "entry", source_route |-> "submit_work", source_machine |-> "external_entry", source_effect |-> "SubmitWork", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "mob", variant |-> "SubmitWork", payload |-> [agent_runtime_id |-> arg_agent_runtime_id, fence_token |-> arg_fence_token, work_id |-> arg_work_id], source_kind |-> "entry", source_route |-> "submit_work", source_machine |-> "external_entry", source_effect |-> "SubmitWork", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "SubmitWork", payload |-> [agent_runtime_id |-> arg_agent_runtime_id, fence_token |-> arg_fence_token, work_id |-> arg_work_id], source_kind |-> "entry", source_route |-> "submit_work", source_machine |-> "external_entry", source_effect |-> "SubmitWork", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_retire_member(arg_agent_runtime_id) ==
    /\ ~([machine |-> "mob", variant |-> "Retire", payload |-> [agent_runtime_id |-> arg_agent_runtime_id], source_kind |-> "entry", source_route |-> "retire_member", source_machine |-> "external_entry", source_effect |-> "Retire", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "mob", variant |-> "Retire", payload |-> [agent_runtime_id |-> arg_agent_runtime_id], source_kind |-> "entry", source_route |-> "retire_member", source_machine |-> "external_entry", source_effect |-> "Retire", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "Retire", payload |-> [agent_runtime_id |-> arg_agent_runtime_id], source_kind |-> "entry", source_route |-> "retire_member", source_machine |-> "external_entry", source_effect |-> "Retire", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_destroy_mob ==
    /\ ~([machine |-> "mob", variant |-> "Destroy", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "destroy_mob", source_machine |-> "external_entry", source_effect |-> "Destroy", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "mob", variant |-> "Destroy", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "destroy_mob", source_machine |-> "external_entry", source_effect |-> "Destroy", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "Destroy", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "destroy_mob", source_machine |-> "external_entry", source_effect |-> "Destroy", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

DeliverQueuedRoute ==
    /\ Len(pending_routes) > 0
    /\ LET route == Head(pending_routes) IN
       /\ pending_routes' = Tail(pending_routes)
       /\ delivered_routes' = delivered_routes \cup {route}
       /\ model_step_count' = model_step_count + 1
       /\ pending_inputs' = AppendIfMissing(pending_inputs, [machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id]}
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, mob_pending_spawn_count, mob_retiring_member_count, mob_wiring_edge_count, mob_task_count, mob_event_subscription_count, mob_active_frame_count, mob_active_loop_count, mob_coordinator_bound, mob_kickoff_pending, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

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

WitnessInjectNext_work_terminal_variants ==
    FALSE

CoreNext ==
    \/ DeliverQueuedRoute
    \/ meerkat_Initialize
    \/ \E arg_session_id \in SessionIdValues : meerkat_RegisterSession(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_UnregisterSession(arg_session_id)
    \/ \E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterAttached(arg_filter, arg_witnesses)
    \/ \E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterRunning(arg_filter, arg_witnesses)
    \/ \E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsAttached(arg_names, arg_witnesses)
    \/ \E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsRunning(arg_names, arg_witnesses)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindings(arg_agent_runtime_id, arg_fence_token, arg_generation)
    \/ \E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextAttached(arg_keep_alive)
    \/ \E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextRunning(arg_keep_alive)
    \/ \E arg_reason \in StringValues : meerkat_NotifyDrainExitedAttached(arg_reason)
    \/ \E arg_reason \in StringValues : meerkat_NotifyDrainExitedRunning(arg_reason)
    \/ \E arg_keys \in SetOfReachabilityKeyValues : \E arg_reachability \in MapReachabilityKeyPeerReachabilityValues : \E arg_last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : meerkat_ReconcileResolvedDirectoryAttached(arg_keys, arg_reachability, arg_last_reason)
    \/ \E arg_keys \in SetOfReachabilityKeyValues : \E arg_reachability \in MapReachabilityKeyPeerReachabilityValues : \E arg_last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : meerkat_ReconcileResolvedDirectoryRunning(arg_keys, arg_reachability, arg_last_reason)
    \/ \E arg_key \in ReachabilityKeyValues : meerkat_RecordSendSucceededAttached(arg_key)
    \/ \E arg_key \in ReachabilityKeyValues : meerkat_RecordSendSucceededRunning(arg_key)
    \/ \E arg_key \in ReachabilityKeyValues : \E arg_reason \in PeerReachabilityReasonValues : meerkat_RecordSendFailedAttached(arg_key, arg_reason)
    \/ \E arg_key \in ReachabilityKeyValues : \E arg_reason \in PeerReachabilityReasonValues : meerkat_RecordSendFailedRunning(arg_key, arg_reason)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : meerkat_BeginRunFromIdle(arg_agent_runtime_id, arg_fence_token, arg_work_id)
    \/ meerkat_InterruptCurrentRun
    \/ meerkat_CancelAfterBoundary
    \/ \E arg_revision \in 0..2 : meerkat_BoundaryAppliedPromote(arg_revision)
    \/ \E arg_revision \in 0..2 : meerkat_BoundaryAppliedNoop(arg_revision)
    \/ \E arg_revision \in 0..2 : meerkat_PublishCommittedVisibleSetAttached(arg_revision)
    \/ \E arg_revision \in 0..2 : meerkat_PublishCommittedVisibleSetRunning(arg_revision)
    \/ \E arg_work_id \in WorkIdValues : meerkat_RunCompleted(arg_work_id)
    \/ \E arg_work_id \in WorkIdValues : meerkat_RunFailed(arg_work_id)
    \/ \E arg_work_id \in WorkIdValues : meerkat_RunCancelled(arg_work_id)
    \/ meerkat_RecoverFromIdle
    \/ meerkat_RecoverFromAttached
    \/ meerkat_RetireRequestedFromIdle
    \/ meerkat_Reset
    \/ meerkat_StopRuntimeExecutor
    \/ meerkat_Destroy
    \/ \E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsIdle(arg_session_id, arg_intents)
    \/ \E arg_session_id \in SessionIdValues : meerkat_ContainsSessionIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_SessionHasExecutorIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_SessionHasCommsIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_OpsLifecycleRegistryIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : \E arg_input_id \in InputIdValues : meerkat_InputStateIdle(arg_session_id, arg_input_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_ListActiveInputsIdle(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_AbortAttached(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_AbortRunning(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_WaitAttached(arg_session_id)
    \/ \E arg_session_id \in SessionIdValues : meerkat_WaitRunning(arg_session_id)
    \/ meerkat_AbortAllAttached
    \/ meerkat_AbortAllRunning
    \/ meerkat_AbortAllRecovering
    \/ meerkat_AbortAllRetired
    \/ meerkat_AbortAllStopped
    \/ meerkat_EnsureDrainRunningAttached
    \/ meerkat_EnsureDrainRunningRunning
    \/ \E arg_runtime_id \in StringValues : meerkat_IngestAttached(arg_runtime_id)
    \/ \E arg_runtime_id \in StringValues : meerkat_IngestRunning(arg_runtime_id)
    \/ \E arg_kind \in StringValues : meerkat_PublishEventAttached(arg_kind)
    \/ \E arg_kind \in StringValues : meerkat_PublishEventRunning(arg_kind)
    \/ \E arg_input_id \in InputIdValues : meerkat_AcceptWithCompletionAttached(arg_input_id)
    \/ \E arg_input_id \in InputIdValues : meerkat_AcceptWithCompletionRunning(arg_input_id)
    \/ \E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeAttached(arg_input_id)
    \/ \E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeRunning(arg_input_id)
    \/ meerkat_ClassifyExternalEnvelopeAttached
    \/ meerkat_ClassifyExternalEnvelopeRunning
    \/ meerkat_ClassifyPlainEventAttached
    \/ meerkat_ClassifyPlainEventRunning
    \/ \E arg_runtime_id \in StringValues : meerkat_RuntimeStateAttached(arg_runtime_id)
    \/ \E arg_runtime_id \in StringValues : meerkat_RuntimeStateRunning(arg_runtime_id)
    \/ \E arg_runtime_id \in StringValues : \E arg_sequence \in 0..2 : meerkat_LoadBoundaryReceiptAttached(arg_runtime_id, arg_sequence)
    \/ \E arg_runtime_id \in StringValues : \E arg_sequence \in 0..2 : meerkat_LoadBoundaryReceiptRunning(arg_runtime_id, arg_sequence)
    \/ \E arg_session_id \in SessionIdValues : meerkat_PrepareAttached(arg_session_id)
    \/ meerkat_StartConversationRunAttached
    \/ meerkat_StartImmediateAppendAttached
    \/ meerkat_StartImmediateContextAttached
    \/ \E arg_input_id \in InputIdValues : \E arg_run_id \in RunIdValues : meerkat_CommitRunning(arg_input_id, arg_run_id)
    \/ \E arg_run_id \in RunIdValues : meerkat_FailRunning(arg_run_id)
    \/ meerkat_AdmitQueuedRunning
    \/ meerkat_AdmitConsumedOnAcceptRunning
    \/ meerkat_StageDrainSnapshotRunning
    \/ meerkat_SupersedeQueuedInputRunning
    \/ meerkat_CoalesceQueuedInputsRunning
    \/ meerkat_SetSilentIntentOverridesRunning
    \/ meerkat_PrimitiveAppliedRunning
    \/ meerkat_LlmReturnedToolCallsRunning
    \/ meerkat_LlmReturnedTerminalRunning
    \/ meerkat_RegisterPendingOpsRunning
    \/ meerkat_ToolCallsResolvedRunning
    \/ meerkat_OpsBarrierSatisfiedRunning
    \/ meerkat_BoundaryContinueRunning
    \/ meerkat_BoundaryCompleteRunning
    \/ meerkat_RecoverableFailureRunning
    \/ meerkat_FatalFailureRunning
    \/ meerkat_RetryRequestedRunning
    \/ meerkat_CancelNowRunning
    \/ meerkat_CancellationObservedRunning
    \/ meerkat_AcknowledgeTerminalRunning
    \/ meerkat_TurnLimitReachedRunning
    \/ meerkat_BudgetExhaustedRunning
    \/ meerkat_TimeBudgetExceededRunning
    \/ meerkat_EnterExtractionRunning
    \/ meerkat_ExtractionValidationPassedRunning
    \/ meerkat_ExtractionValidationFailedRunning
    \/ meerkat_ExtractionStartRunning
    \/ meerkat_ForceCancelNoRunRunning
    \/ meerkat_RegisterOperationRunning
    \/ meerkat_ProvisioningSucceededRunning
    \/ meerkat_ProvisioningFailedRunning
    \/ meerkat_AbortProvisioningRunning
    \/ meerkat_PeerReadyRunning
    \/ meerkat_RegisterWatcherRunning
    \/ meerkat_ProgressReportedRunning
    \/ meerkat_CompleteOperationRunning
    \/ meerkat_FailOperationRunning
    \/ meerkat_CancelOperationRunning
    \/ meerkat_RetireRequestedRunning
    \/ meerkat_RetireCompletedRunning
    \/ meerkat_CollectTerminalRunning
    \/ meerkat_BeginWaitAllRunning
    \/ meerkat_CancelWaitAllRunning
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
    \/ mob_Start
    \/ \E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_Spawn(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeReady(arg_agent_runtime_id, arg_fence_token)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_SubmitWork(arg_agent_runtime_id, arg_fence_token, arg_work_id)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkCompleted(arg_agent_runtime_id, arg_fence_token, arg_work_id)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkFailed(arg_agent_runtime_id, arg_fence_token, arg_work_id)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkCancelled(arg_agent_runtime_id, arg_fence_token, arg_work_id)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_RetireMember(arg_agent_runtime_id, arg_fence_token)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeRetired(arg_agent_runtime_id, arg_fence_token)
    \/ \E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_ResetMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)
    \/ \E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_RespawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)
    \/ mob_MarkCompleted
    \/ mob_DestroyMob
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeDestroyed(arg_agent_runtime_id, arg_fence_token)
    \/ mob_FlowStatusCreating
    \/ mob_FlowStatusRunning
    \/ mob_FlowStatusStopped
    \/ mob_FlowStatusCompleted
    \/ mob_FlowStatusDestroyed
    \/ mob_McpServerStatesCreating
    \/ mob_McpServerStatesRunning
    \/ mob_McpServerStatesStopped
    \/ mob_McpServerStatesCompleted
    \/ mob_McpServerStatesDestroyed
    \/ mob_RosterSnapshotCreating
    \/ mob_RosterSnapshotRunning
    \/ mob_RosterSnapshotStopped
    \/ mob_RosterSnapshotCompleted
    \/ mob_RosterSnapshotDestroyed
    \/ mob_ListMembersCreating
    \/ mob_ListMembersRunning
    \/ mob_ListMembersStopped
    \/ mob_ListMembersCompleted
    \/ mob_ListMembersDestroyed
    \/ mob_ListMembersIncludingRetiringCreating
    \/ mob_ListMembersIncludingRetiringRunning
    \/ mob_ListMembersIncludingRetiringStopped
    \/ mob_ListMembersIncludingRetiringCompleted
    \/ mob_ListMembersIncludingRetiringDestroyed
    \/ mob_ListAllMembersCreating
    \/ mob_ListAllMembersRunning
    \/ mob_ListAllMembersStopped
    \/ mob_ListAllMembersCompleted
    \/ mob_ListAllMembersDestroyed
    \/ mob_MemberStatusCreating
    \/ mob_MemberStatusRunning
    \/ mob_MemberStatusStopped
    \/ mob_MemberStatusCompleted
    \/ mob_MemberStatusDestroyed
    \/ mob_TaskListCreating
    \/ mob_TaskListRunning
    \/ mob_TaskListStopped
    \/ mob_TaskListCompleted
    \/ mob_TaskListDestroyed
    \/ mob_TaskGetCreating
    \/ mob_TaskGetRunning
    \/ mob_TaskGetStopped
    \/ mob_TaskGetCompleted
    \/ mob_TaskGetDestroyed
    \/ mob_PollEventsCreating
    \/ mob_PollEventsRunning
    \/ mob_PollEventsStopped
    \/ mob_PollEventsCompleted
    \/ mob_PollEventsDestroyed
    \/ mob_ReplayAllEventsCreating
    \/ mob_ReplayAllEventsRunning
    \/ mob_ReplayAllEventsStopped
    \/ mob_ReplayAllEventsCompleted
    \/ mob_ReplayAllEventsDestroyed
    \/ mob_RecordOperatorActionProvenanceCreating
    \/ mob_RecordOperatorActionProvenanceRunning
    \/ mob_RecordOperatorActionProvenanceStopped
    \/ mob_RecordOperatorActionProvenanceCompleted
    \/ mob_RecordOperatorActionProvenanceDestroyed
    \/ mob_GetMemberCreating
    \/ mob_GetMemberRunning
    \/ mob_GetMemberStopped
    \/ mob_GetMemberCompleted
    \/ mob_GetMemberDestroyed
    \/ mob_SetSpawnPolicyCreating
    \/ mob_SetSpawnPolicyRunning
    \/ mob_SetSpawnPolicyStopped
    \/ mob_SetSpawnPolicyCompleted
    \/ mob_SetSpawnPolicyDestroyed
    \/ mob_StopRunning
    \/ mob_ResumeStopped
    \/ mob_CompleteRunning
    \/ mob_ResetToRunning
    \/ mob_WireCreating
    \/ mob_WireRunning
    \/ mob_ExternalTurnCreating
    \/ mob_ExternalTurnRunning
    \/ mob_InternalTurnCreating
    \/ mob_InternalTurnRunning
    \/ mob_TaskCreateCreating
    \/ mob_TaskCreateRunning
    \/ mob_TaskUpdateCreating
    \/ mob_TaskUpdateRunning
    \/ mob_ForceCancelCreating
    \/ mob_ForceCancelRunning
    \/ mob_SubscribeAgentEventsCreating
    \/ mob_SubscribeAgentEventsRunning
    \/ mob_SubscribeAgentEventsStopped
    \/ mob_SubscribeAgentEventsCompleted
    \/ mob_SubscribeAgentEventsDestroyed
    \/ mob_SubscribeAllAgentEventsCreating
    \/ mob_SubscribeAllAgentEventsRunning
    \/ mob_SubscribeAllAgentEventsStopped
    \/ mob_SubscribeAllAgentEventsCompleted
    \/ mob_SubscribeAllAgentEventsDestroyed
    \/ mob_SubscribeMobEventsCreating
    \/ mob_SubscribeMobEventsRunning
    \/ mob_SubscribeMobEventsStopped
    \/ mob_SubscribeMobEventsCompleted
    \/ mob_SubscribeMobEventsDestroyed
    \/ mob_ShutdownRunning
    \/ mob_ShutdownCreating
    \/ mob_ShutdownStopped
    \/ mob_ShutdownCompleted
    \/ mob_CancelFlowRunning
    \/ mob_InitializeOrchestratorRunning
    \/ mob_BindCoordinatorRunning
    \/ mob_UnbindCoordinatorRunning
    \/ mob_StageSpawnRunning
    \/ mob_StopOrchestratorRunning
    \/ mob_ResumeOrchestratorRunning
    \/ mob_DestroyOrchestratorRunning
    \/ mob_ForceCancelMemberRunning
    \/ mob_MemberPeerExposedRunning
    \/ mob_MemberTerminalizedRunning
    \/ mob_OperationPeerTrustedRunning
    \/ mob_PeerInputAdmittedRunning
    \/ mob_RuntimeWorkAdmittedRunning
    \/ mob_KickoffFailedRunning
    \/ mob_KickoffCancelledRunning
    \/ mob_KickoffForceCancelledRunning
    \/ mob_RuntimeRunSubmittedRunning
    \/ mob_RuntimeRunCompletedRunning
    \/ mob_RuntimeRunFailedRunning
    \/ mob_RuntimeRunCancelledRunning
    \/ mob_RuntimeStopRequestedRunning
    \/ mob_DispatchStepRunning
    \/ mob_CompleteStepRunning
    \/ mob_RecordStepOutputRunning
    \/ mob_ConditionPassedRunning
    \/ mob_ConditionRejectedRunning
    \/ mob_FailStepRunning
    \/ mob_SkipStepRunning
    \/ mob_ProjectFrameStepStatusRunning
    \/ mob_CancelStepRunning
    \/ mob_RegisterTargetsRunning
    \/ mob_RecordTargetSuccessRunning
    \/ mob_RecordTargetTerminalFailureRunning
    \/ mob_RecordTargetCanceledRunning
    \/ mob_RecordTargetFailureRunning
    \/ mob_NodeExecutionReleasedRunning
    \/ mob_TerminalizeCompletedRunning
    \/ mob_TerminalizeFailedRunning
    \/ mob_TerminalizeCanceledRunning
    \/ mob_CompleteNodeRunning
    \/ mob_RecordNodeOutputRunning
    \/ mob_FailNodeRunning
    \/ mob_SkipNodeRunning
    \/ mob_CancelNodeRunning
    \/ mob_UntilConditionMetRunning
    \/ mob_BeginCleanupRunning
    \/ mob_FinishCleanupRunning
    \/ mob_KickoffStartedRunning
    \/ mob_KickoffCallbackPendingRunning
    \/ mob_RunFlowRunning
    \/ mob_StartFlowRunning
    \/ mob_CreateRunRunning
    \/ mob_StartRunRunning
    \/ mob_RegisterReadyFrameRunning
    \/ mob_UnwireCreating
    \/ mob_UnwireRunning
    \/ mob_RegisterPendingBodyFrameRunning
    \/ mob_CompleteFlowRunning
    \/ mob_StartRootFrameRunning
    \/ mob_StartBodyFrameRunning
    \/ mob_FrameTerminatedRunning
    \/ mob_StartLoopRunning
    \/ mob_BodyFrameStartedRunning
    \/ mob_BodyFrameCompletedRunning
    \/ mob_BodyFrameFailedRunning
    \/ mob_BodyFrameCanceledRunning
    \/ mob_UntilConditionFailedRunning
    \/ mob_CancelLoopRunning
    \/ mob_FinishRunRunning
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireCreating(arg_agent_runtime_id)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireRunning(arg_agent_runtime_id)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireStopped(arg_agent_runtime_id)
    \/ mob_RetireAllCreating
    \/ mob_RetireAllRunning
    \/ mob_RetireAllStopped
    \/ mob_CompleteSpawnRunning
    \/ mob_DestroyFromAny
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RespawnCreating(arg_agent_runtime_id)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RespawnRunning(arg_agent_runtime_id)
    \/ \E arg_work_id \in WorkIdValues : mob_CancelWorkRunning(arg_work_id)
    \/ mob_CancelAllWorkCreating
    \/ mob_CancelAllWorkRunning
    \/ QuiescentStutter

InjectNext ==
    \/ \E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : Inject_spawn_member(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : Inject_submit_work(arg_agent_runtime_id, arg_fence_token, arg_work_id)
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

WitnessNext_work_terminal_variants ==
    \/ CoreNext
    \/ WitnessInjectNext_work_terminal_variants


RouteObserved_binding_request_reaches_meerkat == \E packet \in RoutePackets : packet.route = "binding_request_reaches_meerkat"
RouteCoverage_binding_request_reaches_meerkat == (RouteObserved_binding_request_reaches_meerkat \/ ~RouteObserved_binding_request_reaches_meerkat)
RouteObserved_member_work_reaches_meerkat == \E packet \in RoutePackets : packet.route = "member_work_reaches_meerkat"
RouteCoverage_member_work_reaches_meerkat == (RouteObserved_member_work_reaches_meerkat \/ ~RouteObserved_member_work_reaches_meerkat)
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
RouteObserved_work_completed_reaches_mob == \E packet \in RoutePackets : packet.route = "work_completed_reaches_mob"
RouteCoverage_work_completed_reaches_mob == (RouteObserved_work_completed_reaches_mob \/ ~RouteObserved_work_completed_reaches_mob)
RouteObserved_work_failed_reaches_mob == \E packet \in RoutePackets : packet.route = "work_failed_reaches_mob"
RouteCoverage_work_failed_reaches_mob == (RouteObserved_work_failed_reaches_mob \/ ~RouteObserved_work_failed_reaches_mob)
RouteObserved_work_cancelled_reaches_mob == \E packet \in RoutePackets : packet.route = "work_cancelled_reaches_mob"
RouteCoverage_work_cancelled_reaches_mob == (RouteObserved_work_cancelled_reaches_mob \/ ~RouteObserved_work_cancelled_reaches_mob)
CoverageInstrumentation == RouteCoverage_binding_request_reaches_meerkat /\ RouteCoverage_member_work_reaches_meerkat /\ RouteCoverage_retire_request_reaches_meerkat /\ RouteCoverage_destroy_request_reaches_meerkat /\ RouteCoverage_runtime_bound_reaches_mob /\ RouteCoverage_runtime_retired_reaches_mob /\ RouteCoverage_runtime_destroyed_reaches_mob /\ RouteCoverage_work_completed_reaches_mob /\ RouteCoverage_work_failed_reaches_mob /\ RouteCoverage_work_cancelled_reaches_mob

CiStateConstraint == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(meerkat_resolved_peer_keys) <= 0 /\ Cardinality(DOMAIN meerkat_peer_reachability) <= 0 /\ Cardinality(DOMAIN meerkat_peer_last_reason) <= 0 /\ Cardinality(meerkat_active_requested_deferred_names) <= 0 /\ Cardinality(meerkat_staged_requested_deferred_names) <= 0 /\ Cardinality(DOMAIN meerkat_requested_witnesses) <= 0 /\ Cardinality(DOMAIN meerkat_filter_witnesses) <= 0
DeepStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 2 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(meerkat_resolved_peer_keys) <= 2 /\ Cardinality(DOMAIN meerkat_peer_reachability) <= 2 /\ Cardinality(DOMAIN meerkat_peer_last_reason) <= 2 /\ Cardinality(meerkat_active_requested_deferred_names) <= 2 /\ Cardinality(meerkat_staged_requested_deferred_names) <= 2 /\ Cardinality(DOMAIN meerkat_requested_witnesses) <= 2 /\ Cardinality(DOMAIN meerkat_filter_witnesses) <= 2
WitnessStateConstraint_basic_round_trip == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 14 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 4 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(meerkat_resolved_peer_keys) <= 0 /\ Cardinality(DOMAIN meerkat_peer_reachability) <= 0 /\ Cardinality(DOMAIN meerkat_peer_last_reason) <= 0 /\ Cardinality(meerkat_active_requested_deferred_names) <= 0 /\ Cardinality(meerkat_staged_requested_deferred_names) <= 0 /\ Cardinality(DOMAIN meerkat_requested_witnesses) <= 0 /\ Cardinality(DOMAIN meerkat_filter_witnesses) <= 0
WitnessStateConstraint_retire_runtime_path == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 12 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(meerkat_resolved_peer_keys) <= 0 /\ Cardinality(DOMAIN meerkat_peer_reachability) <= 0 /\ Cardinality(DOMAIN meerkat_peer_last_reason) <= 0 /\ Cardinality(meerkat_active_requested_deferred_names) <= 0 /\ Cardinality(meerkat_staged_requested_deferred_names) <= 0 /\ Cardinality(DOMAIN meerkat_requested_witnesses) <= 0 /\ Cardinality(DOMAIN meerkat_filter_witnesses) <= 0
WitnessStateConstraint_destroy_runtime_path == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 12 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(meerkat_resolved_peer_keys) <= 0 /\ Cardinality(DOMAIN meerkat_peer_reachability) <= 0 /\ Cardinality(DOMAIN meerkat_peer_last_reason) <= 0 /\ Cardinality(meerkat_active_requested_deferred_names) <= 0 /\ Cardinality(meerkat_staged_requested_deferred_names) <= 0 /\ Cardinality(DOMAIN meerkat_requested_witnesses) <= 0 /\ Cardinality(DOMAIN meerkat_filter_witnesses) <= 0
WitnessStateConstraint_work_terminal_variants == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 12 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(meerkat_resolved_peer_keys) <= 0 /\ Cardinality(DOMAIN meerkat_peer_reachability) <= 0 /\ Cardinality(DOMAIN meerkat_peer_last_reason) <= 0 /\ Cardinality(meerkat_active_requested_deferred_names) <= 0 /\ Cardinality(meerkat_staged_requested_deferred_names) <= 0 /\ Cardinality(DOMAIN meerkat_requested_witnesses) <= 0 /\ Cardinality(DOMAIN meerkat_filter_witnesses) <= 0

Spec == Init /\ [][Next]_vars
WitnessSpec_basic_round_trip == WitnessInit_basic_round_trip /\ [] [WitnessNext_basic_round_trip]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(meerkat_Initialize) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSession(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSession(arg_session_id)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterAttached(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterRunning(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsAttached(arg_names, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsRunning(arg_names, arg_witnesses)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindings(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextAttached(arg_keep_alive)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextRunning(arg_keep_alive)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedAttached(arg_reason)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedRunning(arg_reason)) /\ WF_vars(\E arg_keys \in SetOfReachabilityKeyValues : \E arg_reachability \in MapReachabilityKeyPeerReachabilityValues : \E arg_last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : meerkat_ReconcileResolvedDirectoryAttached(arg_keys, arg_reachability, arg_last_reason)) /\ WF_vars(\E arg_keys \in SetOfReachabilityKeyValues : \E arg_reachability \in MapReachabilityKeyPeerReachabilityValues : \E arg_last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : meerkat_ReconcileResolvedDirectoryRunning(arg_keys, arg_reachability, arg_last_reason)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : meerkat_RecordSendSucceededAttached(arg_key)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : meerkat_RecordSendSucceededRunning(arg_key)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : \E arg_reason \in PeerReachabilityReasonValues : meerkat_RecordSendFailedAttached(arg_key, arg_reason)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : \E arg_reason \in PeerReachabilityReasonValues : meerkat_RecordSendFailedRunning(arg_key, arg_reason)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : meerkat_BeginRunFromIdle(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(meerkat_InterruptCurrentRun) /\ WF_vars(meerkat_CancelAfterBoundary) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_BoundaryAppliedPromote(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_BoundaryAppliedNoop(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_PublishCommittedVisibleSetAttached(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_PublishCommittedVisibleSetRunning(arg_revision)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunCompleted(arg_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunFailed(arg_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunCancelled(arg_work_id)) /\ WF_vars(meerkat_RecoverFromIdle) /\ WF_vars(meerkat_RecoverFromAttached) /\ WF_vars(meerkat_RetireRequestedFromIdle) /\ WF_vars(meerkat_Reset) /\ WF_vars(meerkat_StopRuntimeExecutor) /\ WF_vars(meerkat_Destroy) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsIdle(arg_session_id, arg_intents)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_ContainsSessionIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_SessionHasExecutorIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_SessionHasCommsIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_OpsLifecycleRegistryIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_input_id \in InputIdValues : meerkat_InputStateIdle(arg_session_id, arg_input_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_ListActiveInputsIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortRunning(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitRunning(arg_session_id)) /\ WF_vars(meerkat_AbortAllAttached) /\ WF_vars(meerkat_AbortAllRunning) /\ WF_vars(meerkat_AbortAllRecovering) /\ WF_vars(meerkat_AbortAllRetired) /\ WF_vars(meerkat_AbortAllStopped) /\ WF_vars(meerkat_EnsureDrainRunningAttached) /\ WF_vars(meerkat_EnsureDrainRunningRunning) /\ WF_vars(\E arg_runtime_id \in StringValues : meerkat_IngestAttached(arg_runtime_id)) /\ WF_vars(\E arg_runtime_id \in StringValues : meerkat_IngestRunning(arg_runtime_id)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventAttached(arg_kind)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventRunning(arg_kind)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithCompletionAttached(arg_input_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithCompletionRunning(arg_input_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeAttached(arg_input_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeRunning(arg_input_id)) /\ WF_vars(meerkat_ClassifyExternalEnvelopeAttached) /\ WF_vars(meerkat_ClassifyExternalEnvelopeRunning) /\ WF_vars(meerkat_ClassifyPlainEventAttached) /\ WF_vars(meerkat_ClassifyPlainEventRunning) /\ WF_vars(\E arg_runtime_id \in StringValues : meerkat_RuntimeStateAttached(arg_runtime_id)) /\ WF_vars(\E arg_runtime_id \in StringValues : meerkat_RuntimeStateRunning(arg_runtime_id)) /\ WF_vars(\E arg_runtime_id \in StringValues : \E arg_sequence \in 0..2 : meerkat_LoadBoundaryReceiptAttached(arg_runtime_id, arg_sequence)) /\ WF_vars(\E arg_runtime_id \in StringValues : \E arg_sequence \in 0..2 : meerkat_LoadBoundaryReceiptRunning(arg_runtime_id, arg_sequence)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_PrepareAttached(arg_session_id)) /\ WF_vars(meerkat_StartConversationRunAttached) /\ WF_vars(meerkat_StartImmediateAppendAttached) /\ WF_vars(meerkat_StartImmediateContextAttached) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_run_id \in RunIdValues : meerkat_CommitRunning(arg_input_id, arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : meerkat_FailRunning(arg_run_id)) /\ WF_vars(meerkat_AdmitQueuedRunning) /\ WF_vars(meerkat_AdmitConsumedOnAcceptRunning) /\ WF_vars(meerkat_StageDrainSnapshotRunning) /\ WF_vars(meerkat_SupersedeQueuedInputRunning) /\ WF_vars(meerkat_CoalesceQueuedInputsRunning) /\ WF_vars(meerkat_SetSilentIntentOverridesRunning) /\ WF_vars(meerkat_PrimitiveAppliedRunning) /\ WF_vars(meerkat_LlmReturnedToolCallsRunning) /\ WF_vars(meerkat_LlmReturnedTerminalRunning) /\ WF_vars(meerkat_RegisterPendingOpsRunning) /\ WF_vars(meerkat_ToolCallsResolvedRunning) /\ WF_vars(meerkat_OpsBarrierSatisfiedRunning) /\ WF_vars(meerkat_BoundaryContinueRunning) /\ WF_vars(meerkat_BoundaryCompleteRunning) /\ WF_vars(meerkat_RecoverableFailureRunning) /\ WF_vars(meerkat_FatalFailureRunning) /\ WF_vars(meerkat_RetryRequestedRunning) /\ WF_vars(meerkat_CancelNowRunning) /\ WF_vars(meerkat_CancellationObservedRunning) /\ WF_vars(meerkat_AcknowledgeTerminalRunning) /\ WF_vars(meerkat_TurnLimitReachedRunning) /\ WF_vars(meerkat_BudgetExhaustedRunning) /\ WF_vars(meerkat_TimeBudgetExceededRunning) /\ WF_vars(meerkat_EnterExtractionRunning) /\ WF_vars(meerkat_ExtractionValidationPassedRunning) /\ WF_vars(meerkat_ExtractionValidationFailedRunning) /\ WF_vars(meerkat_ExtractionStartRunning) /\ WF_vars(meerkat_ForceCancelNoRunRunning) /\ WF_vars(meerkat_RegisterOperationRunning) /\ WF_vars(meerkat_ProvisioningSucceededRunning) /\ WF_vars(meerkat_ProvisioningFailedRunning) /\ WF_vars(meerkat_AbortProvisioningRunning) /\ WF_vars(meerkat_PeerReadyRunning) /\ WF_vars(meerkat_RegisterWatcherRunning) /\ WF_vars(meerkat_ProgressReportedRunning) /\ WF_vars(meerkat_CompleteOperationRunning) /\ WF_vars(meerkat_FailOperationRunning) /\ WF_vars(meerkat_CancelOperationRunning) /\ WF_vars(meerkat_RetireRequestedRunning) /\ WF_vars(meerkat_RetireCompletedRunning) /\ WF_vars(meerkat_CollectTerminalRunning) /\ WF_vars(meerkat_BeginWaitAllRunning) /\ WF_vars(meerkat_CancelWaitAllRunning) /\ WF_vars(meerkat_StageAddAttached) /\ WF_vars(meerkat_StageAddRunning) /\ WF_vars(meerkat_StageRemoveAttached) /\ WF_vars(meerkat_StageRemoveRunning) /\ WF_vars(meerkat_StageReloadAttached) /\ WF_vars(meerkat_StageReloadRunning) /\ WF_vars(meerkat_ApplySurfaceBoundaryAttached) /\ WF_vars(meerkat_ApplySurfaceBoundaryRunning) /\ WF_vars(meerkat_PendingSucceededAttached) /\ WF_vars(meerkat_PendingSucceededRunning) /\ WF_vars(meerkat_PendingFailedAttached) /\ WF_vars(meerkat_PendingFailedRunning) /\ WF_vars(meerkat_CallStartedAttached) /\ WF_vars(meerkat_CallStartedRunning) /\ WF_vars(meerkat_CallFinishedAttached) /\ WF_vars(meerkat_CallFinishedRunning) /\ WF_vars(meerkat_FinalizeRemovalCleanAttached) /\ WF_vars(meerkat_FinalizeRemovalCleanRunning) /\ WF_vars(meerkat_FinalizeRemovalForcedAttached) /\ WF_vars(meerkat_FinalizeRemovalForcedRunning) /\ WF_vars(meerkat_SnapshotAlignedAttached) /\ WF_vars(meerkat_SnapshotAlignedRunning) /\ WF_vars(meerkat_ShutdownSurfaceAttached) /\ WF_vars(meerkat_ShutdownSurfaceRunning) /\ WF_vars(meerkat_RecycleFromIdleOrRetired) /\ WF_vars(meerkat_RecycleFromAttached) /\ WF_vars(mob_Start) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_Spawn(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeReady(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_SubmitWork(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkCompleted(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkFailed(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkCancelled(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_RetireMember(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeRetired(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_ResetMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_RespawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(mob_MarkCompleted) /\ WF_vars(mob_DestroyMob) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeDestroyed(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(mob_FlowStatusCreating) /\ WF_vars(mob_FlowStatusRunning) /\ WF_vars(mob_FlowStatusStopped) /\ WF_vars(mob_FlowStatusCompleted) /\ WF_vars(mob_FlowStatusDestroyed) /\ WF_vars(mob_McpServerStatesCreating) /\ WF_vars(mob_McpServerStatesRunning) /\ WF_vars(mob_McpServerStatesStopped) /\ WF_vars(mob_McpServerStatesCompleted) /\ WF_vars(mob_McpServerStatesDestroyed) /\ WF_vars(mob_RosterSnapshotCreating) /\ WF_vars(mob_RosterSnapshotRunning) /\ WF_vars(mob_RosterSnapshotStopped) /\ WF_vars(mob_RosterSnapshotCompleted) /\ WF_vars(mob_RosterSnapshotDestroyed) /\ WF_vars(mob_ListMembersCreating) /\ WF_vars(mob_ListMembersRunning) /\ WF_vars(mob_ListMembersStopped) /\ WF_vars(mob_ListMembersCompleted) /\ WF_vars(mob_ListMembersDestroyed) /\ WF_vars(mob_ListMembersIncludingRetiringCreating) /\ WF_vars(mob_ListMembersIncludingRetiringRunning) /\ WF_vars(mob_ListMembersIncludingRetiringStopped) /\ WF_vars(mob_ListMembersIncludingRetiringCompleted) /\ WF_vars(mob_ListMembersIncludingRetiringDestroyed) /\ WF_vars(mob_ListAllMembersCreating) /\ WF_vars(mob_ListAllMembersRunning) /\ WF_vars(mob_ListAllMembersStopped) /\ WF_vars(mob_ListAllMembersCompleted) /\ WF_vars(mob_ListAllMembersDestroyed) /\ WF_vars(mob_MemberStatusCreating) /\ WF_vars(mob_MemberStatusRunning) /\ WF_vars(mob_MemberStatusStopped) /\ WF_vars(mob_MemberStatusCompleted) /\ WF_vars(mob_MemberStatusDestroyed) /\ WF_vars(mob_TaskListCreating) /\ WF_vars(mob_TaskListRunning) /\ WF_vars(mob_TaskListStopped) /\ WF_vars(mob_TaskListCompleted) /\ WF_vars(mob_TaskListDestroyed) /\ WF_vars(mob_TaskGetCreating) /\ WF_vars(mob_TaskGetRunning) /\ WF_vars(mob_TaskGetStopped) /\ WF_vars(mob_TaskGetCompleted) /\ WF_vars(mob_TaskGetDestroyed) /\ WF_vars(mob_PollEventsCreating) /\ WF_vars(mob_PollEventsRunning) /\ WF_vars(mob_PollEventsStopped) /\ WF_vars(mob_PollEventsCompleted) /\ WF_vars(mob_PollEventsDestroyed) /\ WF_vars(mob_ReplayAllEventsCreating) /\ WF_vars(mob_ReplayAllEventsRunning) /\ WF_vars(mob_ReplayAllEventsStopped) /\ WF_vars(mob_ReplayAllEventsCompleted) /\ WF_vars(mob_ReplayAllEventsDestroyed) /\ WF_vars(mob_RecordOperatorActionProvenanceCreating) /\ WF_vars(mob_RecordOperatorActionProvenanceRunning) /\ WF_vars(mob_RecordOperatorActionProvenanceStopped) /\ WF_vars(mob_RecordOperatorActionProvenanceCompleted) /\ WF_vars(mob_RecordOperatorActionProvenanceDestroyed) /\ WF_vars(mob_GetMemberCreating) /\ WF_vars(mob_GetMemberRunning) /\ WF_vars(mob_GetMemberStopped) /\ WF_vars(mob_GetMemberCompleted) /\ WF_vars(mob_GetMemberDestroyed) /\ WF_vars(mob_SetSpawnPolicyCreating) /\ WF_vars(mob_SetSpawnPolicyRunning) /\ WF_vars(mob_SetSpawnPolicyStopped) /\ WF_vars(mob_SetSpawnPolicyCompleted) /\ WF_vars(mob_SetSpawnPolicyDestroyed) /\ WF_vars(mob_StopRunning) /\ WF_vars(mob_ResumeStopped) /\ WF_vars(mob_CompleteRunning) /\ WF_vars(mob_ResetToRunning) /\ WF_vars(mob_WireCreating) /\ WF_vars(mob_WireRunning) /\ WF_vars(mob_ExternalTurnCreating) /\ WF_vars(mob_ExternalTurnRunning) /\ WF_vars(mob_InternalTurnCreating) /\ WF_vars(mob_InternalTurnRunning) /\ WF_vars(mob_TaskCreateCreating) /\ WF_vars(mob_TaskCreateRunning) /\ WF_vars(mob_TaskUpdateCreating) /\ WF_vars(mob_TaskUpdateRunning) /\ WF_vars(mob_ForceCancelCreating) /\ WF_vars(mob_ForceCancelRunning) /\ WF_vars(mob_SubscribeAgentEventsCreating) /\ WF_vars(mob_SubscribeAgentEventsRunning) /\ WF_vars(mob_SubscribeAgentEventsStopped) /\ WF_vars(mob_SubscribeAgentEventsCompleted) /\ WF_vars(mob_SubscribeAgentEventsDestroyed) /\ WF_vars(mob_SubscribeAllAgentEventsCreating) /\ WF_vars(mob_SubscribeAllAgentEventsRunning) /\ WF_vars(mob_SubscribeAllAgentEventsStopped) /\ WF_vars(mob_SubscribeAllAgentEventsCompleted) /\ WF_vars(mob_SubscribeAllAgentEventsDestroyed) /\ WF_vars(mob_SubscribeMobEventsCreating) /\ WF_vars(mob_SubscribeMobEventsRunning) /\ WF_vars(mob_SubscribeMobEventsStopped) /\ WF_vars(mob_SubscribeMobEventsCompleted) /\ WF_vars(mob_SubscribeMobEventsDestroyed) /\ WF_vars(mob_ShutdownRunning) /\ WF_vars(mob_ShutdownCreating) /\ WF_vars(mob_ShutdownStopped) /\ WF_vars(mob_ShutdownCompleted) /\ WF_vars(mob_CancelFlowRunning) /\ WF_vars(mob_InitializeOrchestratorRunning) /\ WF_vars(mob_BindCoordinatorRunning) /\ WF_vars(mob_UnbindCoordinatorRunning) /\ WF_vars(mob_StageSpawnRunning) /\ WF_vars(mob_StopOrchestratorRunning) /\ WF_vars(mob_ResumeOrchestratorRunning) /\ WF_vars(mob_DestroyOrchestratorRunning) /\ WF_vars(mob_ForceCancelMemberRunning) /\ WF_vars(mob_MemberPeerExposedRunning) /\ WF_vars(mob_MemberTerminalizedRunning) /\ WF_vars(mob_OperationPeerTrustedRunning) /\ WF_vars(mob_PeerInputAdmittedRunning) /\ WF_vars(mob_RuntimeWorkAdmittedRunning) /\ WF_vars(mob_KickoffFailedRunning) /\ WF_vars(mob_KickoffCancelledRunning) /\ WF_vars(mob_KickoffForceCancelledRunning) /\ WF_vars(mob_RuntimeRunSubmittedRunning) /\ WF_vars(mob_RuntimeRunCompletedRunning) /\ WF_vars(mob_RuntimeRunFailedRunning) /\ WF_vars(mob_RuntimeRunCancelledRunning) /\ WF_vars(mob_RuntimeStopRequestedRunning) /\ WF_vars(mob_DispatchStepRunning) /\ WF_vars(mob_CompleteStepRunning) /\ WF_vars(mob_RecordStepOutputRunning) /\ WF_vars(mob_ConditionPassedRunning) /\ WF_vars(mob_ConditionRejectedRunning) /\ WF_vars(mob_FailStepRunning) /\ WF_vars(mob_SkipStepRunning) /\ WF_vars(mob_ProjectFrameStepStatusRunning) /\ WF_vars(mob_CancelStepRunning) /\ WF_vars(mob_RegisterTargetsRunning) /\ WF_vars(mob_RecordTargetSuccessRunning) /\ WF_vars(mob_RecordTargetTerminalFailureRunning) /\ WF_vars(mob_RecordTargetCanceledRunning) /\ WF_vars(mob_RecordTargetFailureRunning) /\ WF_vars(mob_NodeExecutionReleasedRunning) /\ WF_vars(mob_TerminalizeCompletedRunning) /\ WF_vars(mob_TerminalizeFailedRunning) /\ WF_vars(mob_TerminalizeCanceledRunning) /\ WF_vars(mob_CompleteNodeRunning) /\ WF_vars(mob_RecordNodeOutputRunning) /\ WF_vars(mob_FailNodeRunning) /\ WF_vars(mob_SkipNodeRunning) /\ WF_vars(mob_CancelNodeRunning) /\ WF_vars(mob_UntilConditionMetRunning) /\ WF_vars(mob_BeginCleanupRunning) /\ WF_vars(mob_FinishCleanupRunning) /\ WF_vars(mob_KickoffStartedRunning) /\ WF_vars(mob_KickoffCallbackPendingRunning) /\ WF_vars(mob_RunFlowRunning) /\ WF_vars(mob_StartFlowRunning) /\ WF_vars(mob_CreateRunRunning) /\ WF_vars(mob_StartRunRunning) /\ WF_vars(mob_RegisterReadyFrameRunning) /\ WF_vars(mob_UnwireCreating) /\ WF_vars(mob_UnwireRunning) /\ WF_vars(mob_RegisterPendingBodyFrameRunning) /\ WF_vars(mob_CompleteFlowRunning) /\ WF_vars(mob_StartRootFrameRunning) /\ WF_vars(mob_StartBodyFrameRunning) /\ WF_vars(mob_FrameTerminatedRunning) /\ WF_vars(mob_StartLoopRunning) /\ WF_vars(mob_BodyFrameStartedRunning) /\ WF_vars(mob_BodyFrameCompletedRunning) /\ WF_vars(mob_BodyFrameFailedRunning) /\ WF_vars(mob_BodyFrameCanceledRunning) /\ WF_vars(mob_UntilConditionFailedRunning) /\ WF_vars(mob_CancelLoopRunning) /\ WF_vars(mob_FinishRunRunning) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireCreating(arg_agent_runtime_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireRunning(arg_agent_runtime_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireStopped(arg_agent_runtime_id)) /\ WF_vars(mob_RetireAllCreating) /\ WF_vars(mob_RetireAllRunning) /\ WF_vars(mob_RetireAllStopped) /\ WF_vars(mob_CompleteSpawnRunning) /\ WF_vars(mob_DestroyFromAny) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RespawnCreating(arg_agent_runtime_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RespawnRunning(arg_agent_runtime_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : mob_CancelWorkRunning(arg_work_id)) /\ WF_vars(mob_CancelAllWorkCreating) /\ WF_vars(mob_CancelAllWorkRunning)
WitnessSpec_retire_runtime_path == WitnessInit_retire_runtime_path /\ [] [WitnessNext_retire_runtime_path]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(meerkat_Initialize) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSession(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSession(arg_session_id)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterAttached(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterRunning(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsAttached(arg_names, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsRunning(arg_names, arg_witnesses)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindings(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextAttached(arg_keep_alive)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextRunning(arg_keep_alive)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedAttached(arg_reason)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedRunning(arg_reason)) /\ WF_vars(\E arg_keys \in SetOfReachabilityKeyValues : \E arg_reachability \in MapReachabilityKeyPeerReachabilityValues : \E arg_last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : meerkat_ReconcileResolvedDirectoryAttached(arg_keys, arg_reachability, arg_last_reason)) /\ WF_vars(\E arg_keys \in SetOfReachabilityKeyValues : \E arg_reachability \in MapReachabilityKeyPeerReachabilityValues : \E arg_last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : meerkat_ReconcileResolvedDirectoryRunning(arg_keys, arg_reachability, arg_last_reason)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : meerkat_RecordSendSucceededAttached(arg_key)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : meerkat_RecordSendSucceededRunning(arg_key)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : \E arg_reason \in PeerReachabilityReasonValues : meerkat_RecordSendFailedAttached(arg_key, arg_reason)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : \E arg_reason \in PeerReachabilityReasonValues : meerkat_RecordSendFailedRunning(arg_key, arg_reason)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : meerkat_BeginRunFromIdle(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(meerkat_InterruptCurrentRun) /\ WF_vars(meerkat_CancelAfterBoundary) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_BoundaryAppliedPromote(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_BoundaryAppliedNoop(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_PublishCommittedVisibleSetAttached(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_PublishCommittedVisibleSetRunning(arg_revision)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunCompleted(arg_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunFailed(arg_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunCancelled(arg_work_id)) /\ WF_vars(meerkat_RecoverFromIdle) /\ WF_vars(meerkat_RecoverFromAttached) /\ WF_vars(meerkat_RetireRequestedFromIdle) /\ WF_vars(meerkat_Reset) /\ WF_vars(meerkat_StopRuntimeExecutor) /\ WF_vars(meerkat_Destroy) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsIdle(arg_session_id, arg_intents)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_ContainsSessionIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_SessionHasExecutorIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_SessionHasCommsIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_OpsLifecycleRegistryIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_input_id \in InputIdValues : meerkat_InputStateIdle(arg_session_id, arg_input_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_ListActiveInputsIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortRunning(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitRunning(arg_session_id)) /\ WF_vars(meerkat_AbortAllAttached) /\ WF_vars(meerkat_AbortAllRunning) /\ WF_vars(meerkat_AbortAllRecovering) /\ WF_vars(meerkat_AbortAllRetired) /\ WF_vars(meerkat_AbortAllStopped) /\ WF_vars(meerkat_EnsureDrainRunningAttached) /\ WF_vars(meerkat_EnsureDrainRunningRunning) /\ WF_vars(\E arg_runtime_id \in StringValues : meerkat_IngestAttached(arg_runtime_id)) /\ WF_vars(\E arg_runtime_id \in StringValues : meerkat_IngestRunning(arg_runtime_id)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventAttached(arg_kind)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventRunning(arg_kind)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithCompletionAttached(arg_input_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithCompletionRunning(arg_input_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeAttached(arg_input_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeRunning(arg_input_id)) /\ WF_vars(meerkat_ClassifyExternalEnvelopeAttached) /\ WF_vars(meerkat_ClassifyExternalEnvelopeRunning) /\ WF_vars(meerkat_ClassifyPlainEventAttached) /\ WF_vars(meerkat_ClassifyPlainEventRunning) /\ WF_vars(\E arg_runtime_id \in StringValues : meerkat_RuntimeStateAttached(arg_runtime_id)) /\ WF_vars(\E arg_runtime_id \in StringValues : meerkat_RuntimeStateRunning(arg_runtime_id)) /\ WF_vars(\E arg_runtime_id \in StringValues : \E arg_sequence \in 0..2 : meerkat_LoadBoundaryReceiptAttached(arg_runtime_id, arg_sequence)) /\ WF_vars(\E arg_runtime_id \in StringValues : \E arg_sequence \in 0..2 : meerkat_LoadBoundaryReceiptRunning(arg_runtime_id, arg_sequence)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_PrepareAttached(arg_session_id)) /\ WF_vars(meerkat_StartConversationRunAttached) /\ WF_vars(meerkat_StartImmediateAppendAttached) /\ WF_vars(meerkat_StartImmediateContextAttached) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_run_id \in RunIdValues : meerkat_CommitRunning(arg_input_id, arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : meerkat_FailRunning(arg_run_id)) /\ WF_vars(meerkat_AdmitQueuedRunning) /\ WF_vars(meerkat_AdmitConsumedOnAcceptRunning) /\ WF_vars(meerkat_StageDrainSnapshotRunning) /\ WF_vars(meerkat_SupersedeQueuedInputRunning) /\ WF_vars(meerkat_CoalesceQueuedInputsRunning) /\ WF_vars(meerkat_SetSilentIntentOverridesRunning) /\ WF_vars(meerkat_PrimitiveAppliedRunning) /\ WF_vars(meerkat_LlmReturnedToolCallsRunning) /\ WF_vars(meerkat_LlmReturnedTerminalRunning) /\ WF_vars(meerkat_RegisterPendingOpsRunning) /\ WF_vars(meerkat_ToolCallsResolvedRunning) /\ WF_vars(meerkat_OpsBarrierSatisfiedRunning) /\ WF_vars(meerkat_BoundaryContinueRunning) /\ WF_vars(meerkat_BoundaryCompleteRunning) /\ WF_vars(meerkat_RecoverableFailureRunning) /\ WF_vars(meerkat_FatalFailureRunning) /\ WF_vars(meerkat_RetryRequestedRunning) /\ WF_vars(meerkat_CancelNowRunning) /\ WF_vars(meerkat_CancellationObservedRunning) /\ WF_vars(meerkat_AcknowledgeTerminalRunning) /\ WF_vars(meerkat_TurnLimitReachedRunning) /\ WF_vars(meerkat_BudgetExhaustedRunning) /\ WF_vars(meerkat_TimeBudgetExceededRunning) /\ WF_vars(meerkat_EnterExtractionRunning) /\ WF_vars(meerkat_ExtractionValidationPassedRunning) /\ WF_vars(meerkat_ExtractionValidationFailedRunning) /\ WF_vars(meerkat_ExtractionStartRunning) /\ WF_vars(meerkat_ForceCancelNoRunRunning) /\ WF_vars(meerkat_RegisterOperationRunning) /\ WF_vars(meerkat_ProvisioningSucceededRunning) /\ WF_vars(meerkat_ProvisioningFailedRunning) /\ WF_vars(meerkat_AbortProvisioningRunning) /\ WF_vars(meerkat_PeerReadyRunning) /\ WF_vars(meerkat_RegisterWatcherRunning) /\ WF_vars(meerkat_ProgressReportedRunning) /\ WF_vars(meerkat_CompleteOperationRunning) /\ WF_vars(meerkat_FailOperationRunning) /\ WF_vars(meerkat_CancelOperationRunning) /\ WF_vars(meerkat_RetireRequestedRunning) /\ WF_vars(meerkat_RetireCompletedRunning) /\ WF_vars(meerkat_CollectTerminalRunning) /\ WF_vars(meerkat_BeginWaitAllRunning) /\ WF_vars(meerkat_CancelWaitAllRunning) /\ WF_vars(meerkat_StageAddAttached) /\ WF_vars(meerkat_StageAddRunning) /\ WF_vars(meerkat_StageRemoveAttached) /\ WF_vars(meerkat_StageRemoveRunning) /\ WF_vars(meerkat_StageReloadAttached) /\ WF_vars(meerkat_StageReloadRunning) /\ WF_vars(meerkat_ApplySurfaceBoundaryAttached) /\ WF_vars(meerkat_ApplySurfaceBoundaryRunning) /\ WF_vars(meerkat_PendingSucceededAttached) /\ WF_vars(meerkat_PendingSucceededRunning) /\ WF_vars(meerkat_PendingFailedAttached) /\ WF_vars(meerkat_PendingFailedRunning) /\ WF_vars(meerkat_CallStartedAttached) /\ WF_vars(meerkat_CallStartedRunning) /\ WF_vars(meerkat_CallFinishedAttached) /\ WF_vars(meerkat_CallFinishedRunning) /\ WF_vars(meerkat_FinalizeRemovalCleanAttached) /\ WF_vars(meerkat_FinalizeRemovalCleanRunning) /\ WF_vars(meerkat_FinalizeRemovalForcedAttached) /\ WF_vars(meerkat_FinalizeRemovalForcedRunning) /\ WF_vars(meerkat_SnapshotAlignedAttached) /\ WF_vars(meerkat_SnapshotAlignedRunning) /\ WF_vars(meerkat_ShutdownSurfaceAttached) /\ WF_vars(meerkat_ShutdownSurfaceRunning) /\ WF_vars(meerkat_RecycleFromIdleOrRetired) /\ WF_vars(meerkat_RecycleFromAttached) /\ WF_vars(mob_Start) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_Spawn(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeReady(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_SubmitWork(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkCompleted(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkFailed(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkCancelled(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_RetireMember(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeRetired(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_ResetMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_RespawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(mob_MarkCompleted) /\ WF_vars(mob_DestroyMob) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeDestroyed(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(mob_FlowStatusCreating) /\ WF_vars(mob_FlowStatusRunning) /\ WF_vars(mob_FlowStatusStopped) /\ WF_vars(mob_FlowStatusCompleted) /\ WF_vars(mob_FlowStatusDestroyed) /\ WF_vars(mob_McpServerStatesCreating) /\ WF_vars(mob_McpServerStatesRunning) /\ WF_vars(mob_McpServerStatesStopped) /\ WF_vars(mob_McpServerStatesCompleted) /\ WF_vars(mob_McpServerStatesDestroyed) /\ WF_vars(mob_RosterSnapshotCreating) /\ WF_vars(mob_RosterSnapshotRunning) /\ WF_vars(mob_RosterSnapshotStopped) /\ WF_vars(mob_RosterSnapshotCompleted) /\ WF_vars(mob_RosterSnapshotDestroyed) /\ WF_vars(mob_ListMembersCreating) /\ WF_vars(mob_ListMembersRunning) /\ WF_vars(mob_ListMembersStopped) /\ WF_vars(mob_ListMembersCompleted) /\ WF_vars(mob_ListMembersDestroyed) /\ WF_vars(mob_ListMembersIncludingRetiringCreating) /\ WF_vars(mob_ListMembersIncludingRetiringRunning) /\ WF_vars(mob_ListMembersIncludingRetiringStopped) /\ WF_vars(mob_ListMembersIncludingRetiringCompleted) /\ WF_vars(mob_ListMembersIncludingRetiringDestroyed) /\ WF_vars(mob_ListAllMembersCreating) /\ WF_vars(mob_ListAllMembersRunning) /\ WF_vars(mob_ListAllMembersStopped) /\ WF_vars(mob_ListAllMembersCompleted) /\ WF_vars(mob_ListAllMembersDestroyed) /\ WF_vars(mob_MemberStatusCreating) /\ WF_vars(mob_MemberStatusRunning) /\ WF_vars(mob_MemberStatusStopped) /\ WF_vars(mob_MemberStatusCompleted) /\ WF_vars(mob_MemberStatusDestroyed) /\ WF_vars(mob_TaskListCreating) /\ WF_vars(mob_TaskListRunning) /\ WF_vars(mob_TaskListStopped) /\ WF_vars(mob_TaskListCompleted) /\ WF_vars(mob_TaskListDestroyed) /\ WF_vars(mob_TaskGetCreating) /\ WF_vars(mob_TaskGetRunning) /\ WF_vars(mob_TaskGetStopped) /\ WF_vars(mob_TaskGetCompleted) /\ WF_vars(mob_TaskGetDestroyed) /\ WF_vars(mob_PollEventsCreating) /\ WF_vars(mob_PollEventsRunning) /\ WF_vars(mob_PollEventsStopped) /\ WF_vars(mob_PollEventsCompleted) /\ WF_vars(mob_PollEventsDestroyed) /\ WF_vars(mob_ReplayAllEventsCreating) /\ WF_vars(mob_ReplayAllEventsRunning) /\ WF_vars(mob_ReplayAllEventsStopped) /\ WF_vars(mob_ReplayAllEventsCompleted) /\ WF_vars(mob_ReplayAllEventsDestroyed) /\ WF_vars(mob_RecordOperatorActionProvenanceCreating) /\ WF_vars(mob_RecordOperatorActionProvenanceRunning) /\ WF_vars(mob_RecordOperatorActionProvenanceStopped) /\ WF_vars(mob_RecordOperatorActionProvenanceCompleted) /\ WF_vars(mob_RecordOperatorActionProvenanceDestroyed) /\ WF_vars(mob_GetMemberCreating) /\ WF_vars(mob_GetMemberRunning) /\ WF_vars(mob_GetMemberStopped) /\ WF_vars(mob_GetMemberCompleted) /\ WF_vars(mob_GetMemberDestroyed) /\ WF_vars(mob_SetSpawnPolicyCreating) /\ WF_vars(mob_SetSpawnPolicyRunning) /\ WF_vars(mob_SetSpawnPolicyStopped) /\ WF_vars(mob_SetSpawnPolicyCompleted) /\ WF_vars(mob_SetSpawnPolicyDestroyed) /\ WF_vars(mob_StopRunning) /\ WF_vars(mob_ResumeStopped) /\ WF_vars(mob_CompleteRunning) /\ WF_vars(mob_ResetToRunning) /\ WF_vars(mob_WireCreating) /\ WF_vars(mob_WireRunning) /\ WF_vars(mob_ExternalTurnCreating) /\ WF_vars(mob_ExternalTurnRunning) /\ WF_vars(mob_InternalTurnCreating) /\ WF_vars(mob_InternalTurnRunning) /\ WF_vars(mob_TaskCreateCreating) /\ WF_vars(mob_TaskCreateRunning) /\ WF_vars(mob_TaskUpdateCreating) /\ WF_vars(mob_TaskUpdateRunning) /\ WF_vars(mob_ForceCancelCreating) /\ WF_vars(mob_ForceCancelRunning) /\ WF_vars(mob_SubscribeAgentEventsCreating) /\ WF_vars(mob_SubscribeAgentEventsRunning) /\ WF_vars(mob_SubscribeAgentEventsStopped) /\ WF_vars(mob_SubscribeAgentEventsCompleted) /\ WF_vars(mob_SubscribeAgentEventsDestroyed) /\ WF_vars(mob_SubscribeAllAgentEventsCreating) /\ WF_vars(mob_SubscribeAllAgentEventsRunning) /\ WF_vars(mob_SubscribeAllAgentEventsStopped) /\ WF_vars(mob_SubscribeAllAgentEventsCompleted) /\ WF_vars(mob_SubscribeAllAgentEventsDestroyed) /\ WF_vars(mob_SubscribeMobEventsCreating) /\ WF_vars(mob_SubscribeMobEventsRunning) /\ WF_vars(mob_SubscribeMobEventsStopped) /\ WF_vars(mob_SubscribeMobEventsCompleted) /\ WF_vars(mob_SubscribeMobEventsDestroyed) /\ WF_vars(mob_ShutdownRunning) /\ WF_vars(mob_ShutdownCreating) /\ WF_vars(mob_ShutdownStopped) /\ WF_vars(mob_ShutdownCompleted) /\ WF_vars(mob_CancelFlowRunning) /\ WF_vars(mob_InitializeOrchestratorRunning) /\ WF_vars(mob_BindCoordinatorRunning) /\ WF_vars(mob_UnbindCoordinatorRunning) /\ WF_vars(mob_StageSpawnRunning) /\ WF_vars(mob_StopOrchestratorRunning) /\ WF_vars(mob_ResumeOrchestratorRunning) /\ WF_vars(mob_DestroyOrchestratorRunning) /\ WF_vars(mob_ForceCancelMemberRunning) /\ WF_vars(mob_MemberPeerExposedRunning) /\ WF_vars(mob_MemberTerminalizedRunning) /\ WF_vars(mob_OperationPeerTrustedRunning) /\ WF_vars(mob_PeerInputAdmittedRunning) /\ WF_vars(mob_RuntimeWorkAdmittedRunning) /\ WF_vars(mob_KickoffFailedRunning) /\ WF_vars(mob_KickoffCancelledRunning) /\ WF_vars(mob_KickoffForceCancelledRunning) /\ WF_vars(mob_RuntimeRunSubmittedRunning) /\ WF_vars(mob_RuntimeRunCompletedRunning) /\ WF_vars(mob_RuntimeRunFailedRunning) /\ WF_vars(mob_RuntimeRunCancelledRunning) /\ WF_vars(mob_RuntimeStopRequestedRunning) /\ WF_vars(mob_DispatchStepRunning) /\ WF_vars(mob_CompleteStepRunning) /\ WF_vars(mob_RecordStepOutputRunning) /\ WF_vars(mob_ConditionPassedRunning) /\ WF_vars(mob_ConditionRejectedRunning) /\ WF_vars(mob_FailStepRunning) /\ WF_vars(mob_SkipStepRunning) /\ WF_vars(mob_ProjectFrameStepStatusRunning) /\ WF_vars(mob_CancelStepRunning) /\ WF_vars(mob_RegisterTargetsRunning) /\ WF_vars(mob_RecordTargetSuccessRunning) /\ WF_vars(mob_RecordTargetTerminalFailureRunning) /\ WF_vars(mob_RecordTargetCanceledRunning) /\ WF_vars(mob_RecordTargetFailureRunning) /\ WF_vars(mob_NodeExecutionReleasedRunning) /\ WF_vars(mob_TerminalizeCompletedRunning) /\ WF_vars(mob_TerminalizeFailedRunning) /\ WF_vars(mob_TerminalizeCanceledRunning) /\ WF_vars(mob_CompleteNodeRunning) /\ WF_vars(mob_RecordNodeOutputRunning) /\ WF_vars(mob_FailNodeRunning) /\ WF_vars(mob_SkipNodeRunning) /\ WF_vars(mob_CancelNodeRunning) /\ WF_vars(mob_UntilConditionMetRunning) /\ WF_vars(mob_BeginCleanupRunning) /\ WF_vars(mob_FinishCleanupRunning) /\ WF_vars(mob_KickoffStartedRunning) /\ WF_vars(mob_KickoffCallbackPendingRunning) /\ WF_vars(mob_RunFlowRunning) /\ WF_vars(mob_StartFlowRunning) /\ WF_vars(mob_CreateRunRunning) /\ WF_vars(mob_StartRunRunning) /\ WF_vars(mob_RegisterReadyFrameRunning) /\ WF_vars(mob_UnwireCreating) /\ WF_vars(mob_UnwireRunning) /\ WF_vars(mob_RegisterPendingBodyFrameRunning) /\ WF_vars(mob_CompleteFlowRunning) /\ WF_vars(mob_StartRootFrameRunning) /\ WF_vars(mob_StartBodyFrameRunning) /\ WF_vars(mob_FrameTerminatedRunning) /\ WF_vars(mob_StartLoopRunning) /\ WF_vars(mob_BodyFrameStartedRunning) /\ WF_vars(mob_BodyFrameCompletedRunning) /\ WF_vars(mob_BodyFrameFailedRunning) /\ WF_vars(mob_BodyFrameCanceledRunning) /\ WF_vars(mob_UntilConditionFailedRunning) /\ WF_vars(mob_CancelLoopRunning) /\ WF_vars(mob_FinishRunRunning) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireCreating(arg_agent_runtime_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireRunning(arg_agent_runtime_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireStopped(arg_agent_runtime_id)) /\ WF_vars(mob_RetireAllCreating) /\ WF_vars(mob_RetireAllRunning) /\ WF_vars(mob_RetireAllStopped) /\ WF_vars(mob_CompleteSpawnRunning) /\ WF_vars(mob_DestroyFromAny) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RespawnCreating(arg_agent_runtime_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RespawnRunning(arg_agent_runtime_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : mob_CancelWorkRunning(arg_work_id)) /\ WF_vars(mob_CancelAllWorkCreating) /\ WF_vars(mob_CancelAllWorkRunning)
WitnessSpec_destroy_runtime_path == WitnessInit_destroy_runtime_path /\ [] [WitnessNext_destroy_runtime_path]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(meerkat_Initialize) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSession(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSession(arg_session_id)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterAttached(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterRunning(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsAttached(arg_names, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsRunning(arg_names, arg_witnesses)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindings(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextAttached(arg_keep_alive)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextRunning(arg_keep_alive)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedAttached(arg_reason)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedRunning(arg_reason)) /\ WF_vars(\E arg_keys \in SetOfReachabilityKeyValues : \E arg_reachability \in MapReachabilityKeyPeerReachabilityValues : \E arg_last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : meerkat_ReconcileResolvedDirectoryAttached(arg_keys, arg_reachability, arg_last_reason)) /\ WF_vars(\E arg_keys \in SetOfReachabilityKeyValues : \E arg_reachability \in MapReachabilityKeyPeerReachabilityValues : \E arg_last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : meerkat_ReconcileResolvedDirectoryRunning(arg_keys, arg_reachability, arg_last_reason)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : meerkat_RecordSendSucceededAttached(arg_key)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : meerkat_RecordSendSucceededRunning(arg_key)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : \E arg_reason \in PeerReachabilityReasonValues : meerkat_RecordSendFailedAttached(arg_key, arg_reason)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : \E arg_reason \in PeerReachabilityReasonValues : meerkat_RecordSendFailedRunning(arg_key, arg_reason)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : meerkat_BeginRunFromIdle(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(meerkat_InterruptCurrentRun) /\ WF_vars(meerkat_CancelAfterBoundary) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_BoundaryAppliedPromote(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_BoundaryAppliedNoop(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_PublishCommittedVisibleSetAttached(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_PublishCommittedVisibleSetRunning(arg_revision)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunCompleted(arg_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunFailed(arg_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunCancelled(arg_work_id)) /\ WF_vars(meerkat_RecoverFromIdle) /\ WF_vars(meerkat_RecoverFromAttached) /\ WF_vars(meerkat_RetireRequestedFromIdle) /\ WF_vars(meerkat_Reset) /\ WF_vars(meerkat_StopRuntimeExecutor) /\ WF_vars(meerkat_Destroy) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsIdle(arg_session_id, arg_intents)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_ContainsSessionIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_SessionHasExecutorIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_SessionHasCommsIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_OpsLifecycleRegistryIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_input_id \in InputIdValues : meerkat_InputStateIdle(arg_session_id, arg_input_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_ListActiveInputsIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortRunning(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitRunning(arg_session_id)) /\ WF_vars(meerkat_AbortAllAttached) /\ WF_vars(meerkat_AbortAllRunning) /\ WF_vars(meerkat_AbortAllRecovering) /\ WF_vars(meerkat_AbortAllRetired) /\ WF_vars(meerkat_AbortAllStopped) /\ WF_vars(meerkat_EnsureDrainRunningAttached) /\ WF_vars(meerkat_EnsureDrainRunningRunning) /\ WF_vars(\E arg_runtime_id \in StringValues : meerkat_IngestAttached(arg_runtime_id)) /\ WF_vars(\E arg_runtime_id \in StringValues : meerkat_IngestRunning(arg_runtime_id)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventAttached(arg_kind)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventRunning(arg_kind)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithCompletionAttached(arg_input_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithCompletionRunning(arg_input_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeAttached(arg_input_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeRunning(arg_input_id)) /\ WF_vars(meerkat_ClassifyExternalEnvelopeAttached) /\ WF_vars(meerkat_ClassifyExternalEnvelopeRunning) /\ WF_vars(meerkat_ClassifyPlainEventAttached) /\ WF_vars(meerkat_ClassifyPlainEventRunning) /\ WF_vars(\E arg_runtime_id \in StringValues : meerkat_RuntimeStateAttached(arg_runtime_id)) /\ WF_vars(\E arg_runtime_id \in StringValues : meerkat_RuntimeStateRunning(arg_runtime_id)) /\ WF_vars(\E arg_runtime_id \in StringValues : \E arg_sequence \in 0..2 : meerkat_LoadBoundaryReceiptAttached(arg_runtime_id, arg_sequence)) /\ WF_vars(\E arg_runtime_id \in StringValues : \E arg_sequence \in 0..2 : meerkat_LoadBoundaryReceiptRunning(arg_runtime_id, arg_sequence)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_PrepareAttached(arg_session_id)) /\ WF_vars(meerkat_StartConversationRunAttached) /\ WF_vars(meerkat_StartImmediateAppendAttached) /\ WF_vars(meerkat_StartImmediateContextAttached) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_run_id \in RunIdValues : meerkat_CommitRunning(arg_input_id, arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : meerkat_FailRunning(arg_run_id)) /\ WF_vars(meerkat_AdmitQueuedRunning) /\ WF_vars(meerkat_AdmitConsumedOnAcceptRunning) /\ WF_vars(meerkat_StageDrainSnapshotRunning) /\ WF_vars(meerkat_SupersedeQueuedInputRunning) /\ WF_vars(meerkat_CoalesceQueuedInputsRunning) /\ WF_vars(meerkat_SetSilentIntentOverridesRunning) /\ WF_vars(meerkat_PrimitiveAppliedRunning) /\ WF_vars(meerkat_LlmReturnedToolCallsRunning) /\ WF_vars(meerkat_LlmReturnedTerminalRunning) /\ WF_vars(meerkat_RegisterPendingOpsRunning) /\ WF_vars(meerkat_ToolCallsResolvedRunning) /\ WF_vars(meerkat_OpsBarrierSatisfiedRunning) /\ WF_vars(meerkat_BoundaryContinueRunning) /\ WF_vars(meerkat_BoundaryCompleteRunning) /\ WF_vars(meerkat_RecoverableFailureRunning) /\ WF_vars(meerkat_FatalFailureRunning) /\ WF_vars(meerkat_RetryRequestedRunning) /\ WF_vars(meerkat_CancelNowRunning) /\ WF_vars(meerkat_CancellationObservedRunning) /\ WF_vars(meerkat_AcknowledgeTerminalRunning) /\ WF_vars(meerkat_TurnLimitReachedRunning) /\ WF_vars(meerkat_BudgetExhaustedRunning) /\ WF_vars(meerkat_TimeBudgetExceededRunning) /\ WF_vars(meerkat_EnterExtractionRunning) /\ WF_vars(meerkat_ExtractionValidationPassedRunning) /\ WF_vars(meerkat_ExtractionValidationFailedRunning) /\ WF_vars(meerkat_ExtractionStartRunning) /\ WF_vars(meerkat_ForceCancelNoRunRunning) /\ WF_vars(meerkat_RegisterOperationRunning) /\ WF_vars(meerkat_ProvisioningSucceededRunning) /\ WF_vars(meerkat_ProvisioningFailedRunning) /\ WF_vars(meerkat_AbortProvisioningRunning) /\ WF_vars(meerkat_PeerReadyRunning) /\ WF_vars(meerkat_RegisterWatcherRunning) /\ WF_vars(meerkat_ProgressReportedRunning) /\ WF_vars(meerkat_CompleteOperationRunning) /\ WF_vars(meerkat_FailOperationRunning) /\ WF_vars(meerkat_CancelOperationRunning) /\ WF_vars(meerkat_RetireRequestedRunning) /\ WF_vars(meerkat_RetireCompletedRunning) /\ WF_vars(meerkat_CollectTerminalRunning) /\ WF_vars(meerkat_BeginWaitAllRunning) /\ WF_vars(meerkat_CancelWaitAllRunning) /\ WF_vars(meerkat_StageAddAttached) /\ WF_vars(meerkat_StageAddRunning) /\ WF_vars(meerkat_StageRemoveAttached) /\ WF_vars(meerkat_StageRemoveRunning) /\ WF_vars(meerkat_StageReloadAttached) /\ WF_vars(meerkat_StageReloadRunning) /\ WF_vars(meerkat_ApplySurfaceBoundaryAttached) /\ WF_vars(meerkat_ApplySurfaceBoundaryRunning) /\ WF_vars(meerkat_PendingSucceededAttached) /\ WF_vars(meerkat_PendingSucceededRunning) /\ WF_vars(meerkat_PendingFailedAttached) /\ WF_vars(meerkat_PendingFailedRunning) /\ WF_vars(meerkat_CallStartedAttached) /\ WF_vars(meerkat_CallStartedRunning) /\ WF_vars(meerkat_CallFinishedAttached) /\ WF_vars(meerkat_CallFinishedRunning) /\ WF_vars(meerkat_FinalizeRemovalCleanAttached) /\ WF_vars(meerkat_FinalizeRemovalCleanRunning) /\ WF_vars(meerkat_FinalizeRemovalForcedAttached) /\ WF_vars(meerkat_FinalizeRemovalForcedRunning) /\ WF_vars(meerkat_SnapshotAlignedAttached) /\ WF_vars(meerkat_SnapshotAlignedRunning) /\ WF_vars(meerkat_ShutdownSurfaceAttached) /\ WF_vars(meerkat_ShutdownSurfaceRunning) /\ WF_vars(meerkat_RecycleFromIdleOrRetired) /\ WF_vars(meerkat_RecycleFromAttached) /\ WF_vars(mob_Start) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_Spawn(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeReady(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_SubmitWork(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkCompleted(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkFailed(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkCancelled(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_RetireMember(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeRetired(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_ResetMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_RespawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(mob_MarkCompleted) /\ WF_vars(mob_DestroyMob) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeDestroyed(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(mob_FlowStatusCreating) /\ WF_vars(mob_FlowStatusRunning) /\ WF_vars(mob_FlowStatusStopped) /\ WF_vars(mob_FlowStatusCompleted) /\ WF_vars(mob_FlowStatusDestroyed) /\ WF_vars(mob_McpServerStatesCreating) /\ WF_vars(mob_McpServerStatesRunning) /\ WF_vars(mob_McpServerStatesStopped) /\ WF_vars(mob_McpServerStatesCompleted) /\ WF_vars(mob_McpServerStatesDestroyed) /\ WF_vars(mob_RosterSnapshotCreating) /\ WF_vars(mob_RosterSnapshotRunning) /\ WF_vars(mob_RosterSnapshotStopped) /\ WF_vars(mob_RosterSnapshotCompleted) /\ WF_vars(mob_RosterSnapshotDestroyed) /\ WF_vars(mob_ListMembersCreating) /\ WF_vars(mob_ListMembersRunning) /\ WF_vars(mob_ListMembersStopped) /\ WF_vars(mob_ListMembersCompleted) /\ WF_vars(mob_ListMembersDestroyed) /\ WF_vars(mob_ListMembersIncludingRetiringCreating) /\ WF_vars(mob_ListMembersIncludingRetiringRunning) /\ WF_vars(mob_ListMembersIncludingRetiringStopped) /\ WF_vars(mob_ListMembersIncludingRetiringCompleted) /\ WF_vars(mob_ListMembersIncludingRetiringDestroyed) /\ WF_vars(mob_ListAllMembersCreating) /\ WF_vars(mob_ListAllMembersRunning) /\ WF_vars(mob_ListAllMembersStopped) /\ WF_vars(mob_ListAllMembersCompleted) /\ WF_vars(mob_ListAllMembersDestroyed) /\ WF_vars(mob_MemberStatusCreating) /\ WF_vars(mob_MemberStatusRunning) /\ WF_vars(mob_MemberStatusStopped) /\ WF_vars(mob_MemberStatusCompleted) /\ WF_vars(mob_MemberStatusDestroyed) /\ WF_vars(mob_TaskListCreating) /\ WF_vars(mob_TaskListRunning) /\ WF_vars(mob_TaskListStopped) /\ WF_vars(mob_TaskListCompleted) /\ WF_vars(mob_TaskListDestroyed) /\ WF_vars(mob_TaskGetCreating) /\ WF_vars(mob_TaskGetRunning) /\ WF_vars(mob_TaskGetStopped) /\ WF_vars(mob_TaskGetCompleted) /\ WF_vars(mob_TaskGetDestroyed) /\ WF_vars(mob_PollEventsCreating) /\ WF_vars(mob_PollEventsRunning) /\ WF_vars(mob_PollEventsStopped) /\ WF_vars(mob_PollEventsCompleted) /\ WF_vars(mob_PollEventsDestroyed) /\ WF_vars(mob_ReplayAllEventsCreating) /\ WF_vars(mob_ReplayAllEventsRunning) /\ WF_vars(mob_ReplayAllEventsStopped) /\ WF_vars(mob_ReplayAllEventsCompleted) /\ WF_vars(mob_ReplayAllEventsDestroyed) /\ WF_vars(mob_RecordOperatorActionProvenanceCreating) /\ WF_vars(mob_RecordOperatorActionProvenanceRunning) /\ WF_vars(mob_RecordOperatorActionProvenanceStopped) /\ WF_vars(mob_RecordOperatorActionProvenanceCompleted) /\ WF_vars(mob_RecordOperatorActionProvenanceDestroyed) /\ WF_vars(mob_GetMemberCreating) /\ WF_vars(mob_GetMemberRunning) /\ WF_vars(mob_GetMemberStopped) /\ WF_vars(mob_GetMemberCompleted) /\ WF_vars(mob_GetMemberDestroyed) /\ WF_vars(mob_SetSpawnPolicyCreating) /\ WF_vars(mob_SetSpawnPolicyRunning) /\ WF_vars(mob_SetSpawnPolicyStopped) /\ WF_vars(mob_SetSpawnPolicyCompleted) /\ WF_vars(mob_SetSpawnPolicyDestroyed) /\ WF_vars(mob_StopRunning) /\ WF_vars(mob_ResumeStopped) /\ WF_vars(mob_CompleteRunning) /\ WF_vars(mob_ResetToRunning) /\ WF_vars(mob_WireCreating) /\ WF_vars(mob_WireRunning) /\ WF_vars(mob_ExternalTurnCreating) /\ WF_vars(mob_ExternalTurnRunning) /\ WF_vars(mob_InternalTurnCreating) /\ WF_vars(mob_InternalTurnRunning) /\ WF_vars(mob_TaskCreateCreating) /\ WF_vars(mob_TaskCreateRunning) /\ WF_vars(mob_TaskUpdateCreating) /\ WF_vars(mob_TaskUpdateRunning) /\ WF_vars(mob_ForceCancelCreating) /\ WF_vars(mob_ForceCancelRunning) /\ WF_vars(mob_SubscribeAgentEventsCreating) /\ WF_vars(mob_SubscribeAgentEventsRunning) /\ WF_vars(mob_SubscribeAgentEventsStopped) /\ WF_vars(mob_SubscribeAgentEventsCompleted) /\ WF_vars(mob_SubscribeAgentEventsDestroyed) /\ WF_vars(mob_SubscribeAllAgentEventsCreating) /\ WF_vars(mob_SubscribeAllAgentEventsRunning) /\ WF_vars(mob_SubscribeAllAgentEventsStopped) /\ WF_vars(mob_SubscribeAllAgentEventsCompleted) /\ WF_vars(mob_SubscribeAllAgentEventsDestroyed) /\ WF_vars(mob_SubscribeMobEventsCreating) /\ WF_vars(mob_SubscribeMobEventsRunning) /\ WF_vars(mob_SubscribeMobEventsStopped) /\ WF_vars(mob_SubscribeMobEventsCompleted) /\ WF_vars(mob_SubscribeMobEventsDestroyed) /\ WF_vars(mob_ShutdownRunning) /\ WF_vars(mob_ShutdownCreating) /\ WF_vars(mob_ShutdownStopped) /\ WF_vars(mob_ShutdownCompleted) /\ WF_vars(mob_CancelFlowRunning) /\ WF_vars(mob_InitializeOrchestratorRunning) /\ WF_vars(mob_BindCoordinatorRunning) /\ WF_vars(mob_UnbindCoordinatorRunning) /\ WF_vars(mob_StageSpawnRunning) /\ WF_vars(mob_StopOrchestratorRunning) /\ WF_vars(mob_ResumeOrchestratorRunning) /\ WF_vars(mob_DestroyOrchestratorRunning) /\ WF_vars(mob_ForceCancelMemberRunning) /\ WF_vars(mob_MemberPeerExposedRunning) /\ WF_vars(mob_MemberTerminalizedRunning) /\ WF_vars(mob_OperationPeerTrustedRunning) /\ WF_vars(mob_PeerInputAdmittedRunning) /\ WF_vars(mob_RuntimeWorkAdmittedRunning) /\ WF_vars(mob_KickoffFailedRunning) /\ WF_vars(mob_KickoffCancelledRunning) /\ WF_vars(mob_KickoffForceCancelledRunning) /\ WF_vars(mob_RuntimeRunSubmittedRunning) /\ WF_vars(mob_RuntimeRunCompletedRunning) /\ WF_vars(mob_RuntimeRunFailedRunning) /\ WF_vars(mob_RuntimeRunCancelledRunning) /\ WF_vars(mob_RuntimeStopRequestedRunning) /\ WF_vars(mob_DispatchStepRunning) /\ WF_vars(mob_CompleteStepRunning) /\ WF_vars(mob_RecordStepOutputRunning) /\ WF_vars(mob_ConditionPassedRunning) /\ WF_vars(mob_ConditionRejectedRunning) /\ WF_vars(mob_FailStepRunning) /\ WF_vars(mob_SkipStepRunning) /\ WF_vars(mob_ProjectFrameStepStatusRunning) /\ WF_vars(mob_CancelStepRunning) /\ WF_vars(mob_RegisterTargetsRunning) /\ WF_vars(mob_RecordTargetSuccessRunning) /\ WF_vars(mob_RecordTargetTerminalFailureRunning) /\ WF_vars(mob_RecordTargetCanceledRunning) /\ WF_vars(mob_RecordTargetFailureRunning) /\ WF_vars(mob_NodeExecutionReleasedRunning) /\ WF_vars(mob_TerminalizeCompletedRunning) /\ WF_vars(mob_TerminalizeFailedRunning) /\ WF_vars(mob_TerminalizeCanceledRunning) /\ WF_vars(mob_CompleteNodeRunning) /\ WF_vars(mob_RecordNodeOutputRunning) /\ WF_vars(mob_FailNodeRunning) /\ WF_vars(mob_SkipNodeRunning) /\ WF_vars(mob_CancelNodeRunning) /\ WF_vars(mob_UntilConditionMetRunning) /\ WF_vars(mob_BeginCleanupRunning) /\ WF_vars(mob_FinishCleanupRunning) /\ WF_vars(mob_KickoffStartedRunning) /\ WF_vars(mob_KickoffCallbackPendingRunning) /\ WF_vars(mob_RunFlowRunning) /\ WF_vars(mob_StartFlowRunning) /\ WF_vars(mob_CreateRunRunning) /\ WF_vars(mob_StartRunRunning) /\ WF_vars(mob_RegisterReadyFrameRunning) /\ WF_vars(mob_UnwireCreating) /\ WF_vars(mob_UnwireRunning) /\ WF_vars(mob_RegisterPendingBodyFrameRunning) /\ WF_vars(mob_CompleteFlowRunning) /\ WF_vars(mob_StartRootFrameRunning) /\ WF_vars(mob_StartBodyFrameRunning) /\ WF_vars(mob_FrameTerminatedRunning) /\ WF_vars(mob_StartLoopRunning) /\ WF_vars(mob_BodyFrameStartedRunning) /\ WF_vars(mob_BodyFrameCompletedRunning) /\ WF_vars(mob_BodyFrameFailedRunning) /\ WF_vars(mob_BodyFrameCanceledRunning) /\ WF_vars(mob_UntilConditionFailedRunning) /\ WF_vars(mob_CancelLoopRunning) /\ WF_vars(mob_FinishRunRunning) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireCreating(arg_agent_runtime_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireRunning(arg_agent_runtime_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireStopped(arg_agent_runtime_id)) /\ WF_vars(mob_RetireAllCreating) /\ WF_vars(mob_RetireAllRunning) /\ WF_vars(mob_RetireAllStopped) /\ WF_vars(mob_CompleteSpawnRunning) /\ WF_vars(mob_DestroyFromAny) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RespawnCreating(arg_agent_runtime_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RespawnRunning(arg_agent_runtime_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : mob_CancelWorkRunning(arg_work_id)) /\ WF_vars(mob_CancelAllWorkCreating) /\ WF_vars(mob_CancelAllWorkRunning)
WitnessSpec_work_terminal_variants == WitnessInit_work_terminal_variants /\ [] [WitnessNext_work_terminal_variants]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(meerkat_Initialize) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSession(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSession(arg_session_id)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterAttached(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterRunning(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsAttached(arg_names, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsRunning(arg_names, arg_witnesses)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindings(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextAttached(arg_keep_alive)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextRunning(arg_keep_alive)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedAttached(arg_reason)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedRunning(arg_reason)) /\ WF_vars(\E arg_keys \in SetOfReachabilityKeyValues : \E arg_reachability \in MapReachabilityKeyPeerReachabilityValues : \E arg_last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : meerkat_ReconcileResolvedDirectoryAttached(arg_keys, arg_reachability, arg_last_reason)) /\ WF_vars(\E arg_keys \in SetOfReachabilityKeyValues : \E arg_reachability \in MapReachabilityKeyPeerReachabilityValues : \E arg_last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : meerkat_ReconcileResolvedDirectoryRunning(arg_keys, arg_reachability, arg_last_reason)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : meerkat_RecordSendSucceededAttached(arg_key)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : meerkat_RecordSendSucceededRunning(arg_key)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : \E arg_reason \in PeerReachabilityReasonValues : meerkat_RecordSendFailedAttached(arg_key, arg_reason)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : \E arg_reason \in PeerReachabilityReasonValues : meerkat_RecordSendFailedRunning(arg_key, arg_reason)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : meerkat_BeginRunFromIdle(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(meerkat_InterruptCurrentRun) /\ WF_vars(meerkat_CancelAfterBoundary) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_BoundaryAppliedPromote(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_BoundaryAppliedNoop(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_PublishCommittedVisibleSetAttached(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_PublishCommittedVisibleSetRunning(arg_revision)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunCompleted(arg_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunFailed(arg_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunCancelled(arg_work_id)) /\ WF_vars(meerkat_RecoverFromIdle) /\ WF_vars(meerkat_RecoverFromAttached) /\ WF_vars(meerkat_RetireRequestedFromIdle) /\ WF_vars(meerkat_Reset) /\ WF_vars(meerkat_StopRuntimeExecutor) /\ WF_vars(meerkat_Destroy) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_EnsureSessionWithExecutorIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_intents \in SetOfStringValues : meerkat_SetSilentIntentsIdle(arg_session_id, arg_intents)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_ContainsSessionIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_SessionHasExecutorIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_SessionHasCommsIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_OpsLifecycleRegistryIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : \E arg_input_id \in InputIdValues : meerkat_InputStateIdle(arg_session_id, arg_input_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_ListActiveInputsIdle(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_AbortRunning(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitAttached(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_WaitRunning(arg_session_id)) /\ WF_vars(meerkat_AbortAllAttached) /\ WF_vars(meerkat_AbortAllRunning) /\ WF_vars(meerkat_AbortAllRecovering) /\ WF_vars(meerkat_AbortAllRetired) /\ WF_vars(meerkat_AbortAllStopped) /\ WF_vars(meerkat_EnsureDrainRunningAttached) /\ WF_vars(meerkat_EnsureDrainRunningRunning) /\ WF_vars(\E arg_runtime_id \in StringValues : meerkat_IngestAttached(arg_runtime_id)) /\ WF_vars(\E arg_runtime_id \in StringValues : meerkat_IngestRunning(arg_runtime_id)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventAttached(arg_kind)) /\ WF_vars(\E arg_kind \in StringValues : meerkat_PublishEventRunning(arg_kind)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithCompletionAttached(arg_input_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithCompletionRunning(arg_input_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeAttached(arg_input_id)) /\ WF_vars(\E arg_input_id \in InputIdValues : meerkat_AcceptWithoutWakeRunning(arg_input_id)) /\ WF_vars(meerkat_ClassifyExternalEnvelopeAttached) /\ WF_vars(meerkat_ClassifyExternalEnvelopeRunning) /\ WF_vars(meerkat_ClassifyPlainEventAttached) /\ WF_vars(meerkat_ClassifyPlainEventRunning) /\ WF_vars(\E arg_runtime_id \in StringValues : meerkat_RuntimeStateAttached(arg_runtime_id)) /\ WF_vars(\E arg_runtime_id \in StringValues : meerkat_RuntimeStateRunning(arg_runtime_id)) /\ WF_vars(\E arg_runtime_id \in StringValues : \E arg_sequence \in 0..2 : meerkat_LoadBoundaryReceiptAttached(arg_runtime_id, arg_sequence)) /\ WF_vars(\E arg_runtime_id \in StringValues : \E arg_sequence \in 0..2 : meerkat_LoadBoundaryReceiptRunning(arg_runtime_id, arg_sequence)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_PrepareAttached(arg_session_id)) /\ WF_vars(meerkat_StartConversationRunAttached) /\ WF_vars(meerkat_StartImmediateAppendAttached) /\ WF_vars(meerkat_StartImmediateContextAttached) /\ WF_vars(\E arg_input_id \in InputIdValues : \E arg_run_id \in RunIdValues : meerkat_CommitRunning(arg_input_id, arg_run_id)) /\ WF_vars(\E arg_run_id \in RunIdValues : meerkat_FailRunning(arg_run_id)) /\ WF_vars(meerkat_AdmitQueuedRunning) /\ WF_vars(meerkat_AdmitConsumedOnAcceptRunning) /\ WF_vars(meerkat_StageDrainSnapshotRunning) /\ WF_vars(meerkat_SupersedeQueuedInputRunning) /\ WF_vars(meerkat_CoalesceQueuedInputsRunning) /\ WF_vars(meerkat_SetSilentIntentOverridesRunning) /\ WF_vars(meerkat_PrimitiveAppliedRunning) /\ WF_vars(meerkat_LlmReturnedToolCallsRunning) /\ WF_vars(meerkat_LlmReturnedTerminalRunning) /\ WF_vars(meerkat_RegisterPendingOpsRunning) /\ WF_vars(meerkat_ToolCallsResolvedRunning) /\ WF_vars(meerkat_OpsBarrierSatisfiedRunning) /\ WF_vars(meerkat_BoundaryContinueRunning) /\ WF_vars(meerkat_BoundaryCompleteRunning) /\ WF_vars(meerkat_RecoverableFailureRunning) /\ WF_vars(meerkat_FatalFailureRunning) /\ WF_vars(meerkat_RetryRequestedRunning) /\ WF_vars(meerkat_CancelNowRunning) /\ WF_vars(meerkat_CancellationObservedRunning) /\ WF_vars(meerkat_AcknowledgeTerminalRunning) /\ WF_vars(meerkat_TurnLimitReachedRunning) /\ WF_vars(meerkat_BudgetExhaustedRunning) /\ WF_vars(meerkat_TimeBudgetExceededRunning) /\ WF_vars(meerkat_EnterExtractionRunning) /\ WF_vars(meerkat_ExtractionValidationPassedRunning) /\ WF_vars(meerkat_ExtractionValidationFailedRunning) /\ WF_vars(meerkat_ExtractionStartRunning) /\ WF_vars(meerkat_ForceCancelNoRunRunning) /\ WF_vars(meerkat_RegisterOperationRunning) /\ WF_vars(meerkat_ProvisioningSucceededRunning) /\ WF_vars(meerkat_ProvisioningFailedRunning) /\ WF_vars(meerkat_AbortProvisioningRunning) /\ WF_vars(meerkat_PeerReadyRunning) /\ WF_vars(meerkat_RegisterWatcherRunning) /\ WF_vars(meerkat_ProgressReportedRunning) /\ WF_vars(meerkat_CompleteOperationRunning) /\ WF_vars(meerkat_FailOperationRunning) /\ WF_vars(meerkat_CancelOperationRunning) /\ WF_vars(meerkat_RetireRequestedRunning) /\ WF_vars(meerkat_RetireCompletedRunning) /\ WF_vars(meerkat_CollectTerminalRunning) /\ WF_vars(meerkat_BeginWaitAllRunning) /\ WF_vars(meerkat_CancelWaitAllRunning) /\ WF_vars(meerkat_StageAddAttached) /\ WF_vars(meerkat_StageAddRunning) /\ WF_vars(meerkat_StageRemoveAttached) /\ WF_vars(meerkat_StageRemoveRunning) /\ WF_vars(meerkat_StageReloadAttached) /\ WF_vars(meerkat_StageReloadRunning) /\ WF_vars(meerkat_ApplySurfaceBoundaryAttached) /\ WF_vars(meerkat_ApplySurfaceBoundaryRunning) /\ WF_vars(meerkat_PendingSucceededAttached) /\ WF_vars(meerkat_PendingSucceededRunning) /\ WF_vars(meerkat_PendingFailedAttached) /\ WF_vars(meerkat_PendingFailedRunning) /\ WF_vars(meerkat_CallStartedAttached) /\ WF_vars(meerkat_CallStartedRunning) /\ WF_vars(meerkat_CallFinishedAttached) /\ WF_vars(meerkat_CallFinishedRunning) /\ WF_vars(meerkat_FinalizeRemovalCleanAttached) /\ WF_vars(meerkat_FinalizeRemovalCleanRunning) /\ WF_vars(meerkat_FinalizeRemovalForcedAttached) /\ WF_vars(meerkat_FinalizeRemovalForcedRunning) /\ WF_vars(meerkat_SnapshotAlignedAttached) /\ WF_vars(meerkat_SnapshotAlignedRunning) /\ WF_vars(meerkat_ShutdownSurfaceAttached) /\ WF_vars(meerkat_ShutdownSurfaceRunning) /\ WF_vars(meerkat_RecycleFromIdleOrRetired) /\ WF_vars(meerkat_RecycleFromAttached) /\ WF_vars(mob_Start) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_Spawn(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeReady(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_SubmitWork(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkCompleted(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkFailed(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkCancelled(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_RetireMember(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeRetired(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_ResetMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_RespawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(mob_MarkCompleted) /\ WF_vars(mob_DestroyMob) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeDestroyed(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(mob_FlowStatusCreating) /\ WF_vars(mob_FlowStatusRunning) /\ WF_vars(mob_FlowStatusStopped) /\ WF_vars(mob_FlowStatusCompleted) /\ WF_vars(mob_FlowStatusDestroyed) /\ WF_vars(mob_McpServerStatesCreating) /\ WF_vars(mob_McpServerStatesRunning) /\ WF_vars(mob_McpServerStatesStopped) /\ WF_vars(mob_McpServerStatesCompleted) /\ WF_vars(mob_McpServerStatesDestroyed) /\ WF_vars(mob_RosterSnapshotCreating) /\ WF_vars(mob_RosterSnapshotRunning) /\ WF_vars(mob_RosterSnapshotStopped) /\ WF_vars(mob_RosterSnapshotCompleted) /\ WF_vars(mob_RosterSnapshotDestroyed) /\ WF_vars(mob_ListMembersCreating) /\ WF_vars(mob_ListMembersRunning) /\ WF_vars(mob_ListMembersStopped) /\ WF_vars(mob_ListMembersCompleted) /\ WF_vars(mob_ListMembersDestroyed) /\ WF_vars(mob_ListMembersIncludingRetiringCreating) /\ WF_vars(mob_ListMembersIncludingRetiringRunning) /\ WF_vars(mob_ListMembersIncludingRetiringStopped) /\ WF_vars(mob_ListMembersIncludingRetiringCompleted) /\ WF_vars(mob_ListMembersIncludingRetiringDestroyed) /\ WF_vars(mob_ListAllMembersCreating) /\ WF_vars(mob_ListAllMembersRunning) /\ WF_vars(mob_ListAllMembersStopped) /\ WF_vars(mob_ListAllMembersCompleted) /\ WF_vars(mob_ListAllMembersDestroyed) /\ WF_vars(mob_MemberStatusCreating) /\ WF_vars(mob_MemberStatusRunning) /\ WF_vars(mob_MemberStatusStopped) /\ WF_vars(mob_MemberStatusCompleted) /\ WF_vars(mob_MemberStatusDestroyed) /\ WF_vars(mob_TaskListCreating) /\ WF_vars(mob_TaskListRunning) /\ WF_vars(mob_TaskListStopped) /\ WF_vars(mob_TaskListCompleted) /\ WF_vars(mob_TaskListDestroyed) /\ WF_vars(mob_TaskGetCreating) /\ WF_vars(mob_TaskGetRunning) /\ WF_vars(mob_TaskGetStopped) /\ WF_vars(mob_TaskGetCompleted) /\ WF_vars(mob_TaskGetDestroyed) /\ WF_vars(mob_PollEventsCreating) /\ WF_vars(mob_PollEventsRunning) /\ WF_vars(mob_PollEventsStopped) /\ WF_vars(mob_PollEventsCompleted) /\ WF_vars(mob_PollEventsDestroyed) /\ WF_vars(mob_ReplayAllEventsCreating) /\ WF_vars(mob_ReplayAllEventsRunning) /\ WF_vars(mob_ReplayAllEventsStopped) /\ WF_vars(mob_ReplayAllEventsCompleted) /\ WF_vars(mob_ReplayAllEventsDestroyed) /\ WF_vars(mob_RecordOperatorActionProvenanceCreating) /\ WF_vars(mob_RecordOperatorActionProvenanceRunning) /\ WF_vars(mob_RecordOperatorActionProvenanceStopped) /\ WF_vars(mob_RecordOperatorActionProvenanceCompleted) /\ WF_vars(mob_RecordOperatorActionProvenanceDestroyed) /\ WF_vars(mob_GetMemberCreating) /\ WF_vars(mob_GetMemberRunning) /\ WF_vars(mob_GetMemberStopped) /\ WF_vars(mob_GetMemberCompleted) /\ WF_vars(mob_GetMemberDestroyed) /\ WF_vars(mob_SetSpawnPolicyCreating) /\ WF_vars(mob_SetSpawnPolicyRunning) /\ WF_vars(mob_SetSpawnPolicyStopped) /\ WF_vars(mob_SetSpawnPolicyCompleted) /\ WF_vars(mob_SetSpawnPolicyDestroyed) /\ WF_vars(mob_StopRunning) /\ WF_vars(mob_ResumeStopped) /\ WF_vars(mob_CompleteRunning) /\ WF_vars(mob_ResetToRunning) /\ WF_vars(mob_WireCreating) /\ WF_vars(mob_WireRunning) /\ WF_vars(mob_ExternalTurnCreating) /\ WF_vars(mob_ExternalTurnRunning) /\ WF_vars(mob_InternalTurnCreating) /\ WF_vars(mob_InternalTurnRunning) /\ WF_vars(mob_TaskCreateCreating) /\ WF_vars(mob_TaskCreateRunning) /\ WF_vars(mob_TaskUpdateCreating) /\ WF_vars(mob_TaskUpdateRunning) /\ WF_vars(mob_ForceCancelCreating) /\ WF_vars(mob_ForceCancelRunning) /\ WF_vars(mob_SubscribeAgentEventsCreating) /\ WF_vars(mob_SubscribeAgentEventsRunning) /\ WF_vars(mob_SubscribeAgentEventsStopped) /\ WF_vars(mob_SubscribeAgentEventsCompleted) /\ WF_vars(mob_SubscribeAgentEventsDestroyed) /\ WF_vars(mob_SubscribeAllAgentEventsCreating) /\ WF_vars(mob_SubscribeAllAgentEventsRunning) /\ WF_vars(mob_SubscribeAllAgentEventsStopped) /\ WF_vars(mob_SubscribeAllAgentEventsCompleted) /\ WF_vars(mob_SubscribeAllAgentEventsDestroyed) /\ WF_vars(mob_SubscribeMobEventsCreating) /\ WF_vars(mob_SubscribeMobEventsRunning) /\ WF_vars(mob_SubscribeMobEventsStopped) /\ WF_vars(mob_SubscribeMobEventsCompleted) /\ WF_vars(mob_SubscribeMobEventsDestroyed) /\ WF_vars(mob_ShutdownRunning) /\ WF_vars(mob_ShutdownCreating) /\ WF_vars(mob_ShutdownStopped) /\ WF_vars(mob_ShutdownCompleted) /\ WF_vars(mob_CancelFlowRunning) /\ WF_vars(mob_InitializeOrchestratorRunning) /\ WF_vars(mob_BindCoordinatorRunning) /\ WF_vars(mob_UnbindCoordinatorRunning) /\ WF_vars(mob_StageSpawnRunning) /\ WF_vars(mob_StopOrchestratorRunning) /\ WF_vars(mob_ResumeOrchestratorRunning) /\ WF_vars(mob_DestroyOrchestratorRunning) /\ WF_vars(mob_ForceCancelMemberRunning) /\ WF_vars(mob_MemberPeerExposedRunning) /\ WF_vars(mob_MemberTerminalizedRunning) /\ WF_vars(mob_OperationPeerTrustedRunning) /\ WF_vars(mob_PeerInputAdmittedRunning) /\ WF_vars(mob_RuntimeWorkAdmittedRunning) /\ WF_vars(mob_KickoffFailedRunning) /\ WF_vars(mob_KickoffCancelledRunning) /\ WF_vars(mob_KickoffForceCancelledRunning) /\ WF_vars(mob_RuntimeRunSubmittedRunning) /\ WF_vars(mob_RuntimeRunCompletedRunning) /\ WF_vars(mob_RuntimeRunFailedRunning) /\ WF_vars(mob_RuntimeRunCancelledRunning) /\ WF_vars(mob_RuntimeStopRequestedRunning) /\ WF_vars(mob_DispatchStepRunning) /\ WF_vars(mob_CompleteStepRunning) /\ WF_vars(mob_RecordStepOutputRunning) /\ WF_vars(mob_ConditionPassedRunning) /\ WF_vars(mob_ConditionRejectedRunning) /\ WF_vars(mob_FailStepRunning) /\ WF_vars(mob_SkipStepRunning) /\ WF_vars(mob_ProjectFrameStepStatusRunning) /\ WF_vars(mob_CancelStepRunning) /\ WF_vars(mob_RegisterTargetsRunning) /\ WF_vars(mob_RecordTargetSuccessRunning) /\ WF_vars(mob_RecordTargetTerminalFailureRunning) /\ WF_vars(mob_RecordTargetCanceledRunning) /\ WF_vars(mob_RecordTargetFailureRunning) /\ WF_vars(mob_NodeExecutionReleasedRunning) /\ WF_vars(mob_TerminalizeCompletedRunning) /\ WF_vars(mob_TerminalizeFailedRunning) /\ WF_vars(mob_TerminalizeCanceledRunning) /\ WF_vars(mob_CompleteNodeRunning) /\ WF_vars(mob_RecordNodeOutputRunning) /\ WF_vars(mob_FailNodeRunning) /\ WF_vars(mob_SkipNodeRunning) /\ WF_vars(mob_CancelNodeRunning) /\ WF_vars(mob_UntilConditionMetRunning) /\ WF_vars(mob_BeginCleanupRunning) /\ WF_vars(mob_FinishCleanupRunning) /\ WF_vars(mob_KickoffStartedRunning) /\ WF_vars(mob_KickoffCallbackPendingRunning) /\ WF_vars(mob_RunFlowRunning) /\ WF_vars(mob_StartFlowRunning) /\ WF_vars(mob_CreateRunRunning) /\ WF_vars(mob_StartRunRunning) /\ WF_vars(mob_RegisterReadyFrameRunning) /\ WF_vars(mob_UnwireCreating) /\ WF_vars(mob_UnwireRunning) /\ WF_vars(mob_RegisterPendingBodyFrameRunning) /\ WF_vars(mob_CompleteFlowRunning) /\ WF_vars(mob_StartRootFrameRunning) /\ WF_vars(mob_StartBodyFrameRunning) /\ WF_vars(mob_FrameTerminatedRunning) /\ WF_vars(mob_StartLoopRunning) /\ WF_vars(mob_BodyFrameStartedRunning) /\ WF_vars(mob_BodyFrameCompletedRunning) /\ WF_vars(mob_BodyFrameFailedRunning) /\ WF_vars(mob_BodyFrameCanceledRunning) /\ WF_vars(mob_UntilConditionFailedRunning) /\ WF_vars(mob_CancelLoopRunning) /\ WF_vars(mob_FinishRunRunning) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireCreating(arg_agent_runtime_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireRunning(arg_agent_runtime_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RetireStopped(arg_agent_runtime_id)) /\ WF_vars(mob_RetireAllCreating) /\ WF_vars(mob_RetireAllRunning) /\ WF_vars(mob_RetireAllStopped) /\ WF_vars(mob_CompleteSpawnRunning) /\ WF_vars(mob_DestroyFromAny) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RespawnCreating(arg_agent_runtime_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : mob_RespawnRunning(arg_agent_runtime_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : mob_CancelWorkRunning(arg_work_id)) /\ WF_vars(mob_CancelAllWorkCreating) /\ WF_vars(mob_CancelAllWorkRunning)

WitnessRouteObserved_basic_round_trip_binding_request_reaches_meerkat == <> RouteObserved_binding_request_reaches_meerkat
WitnessRouteObserved_basic_round_trip_member_work_reaches_meerkat == <> RouteObserved_member_work_reaches_meerkat
WitnessRouteObserved_basic_round_trip_runtime_bound_reaches_mob == <> RouteObserved_runtime_bound_reaches_mob
WitnessRouteObserved_basic_round_trip_work_completed_reaches_mob == <> RouteObserved_work_completed_reaches_mob
WitnessRouteObserved_retire_runtime_path_retire_request_reaches_meerkat == <> RouteObserved_retire_request_reaches_meerkat
WitnessRouteObserved_retire_runtime_path_runtime_retired_reaches_mob == <> RouteObserved_runtime_retired_reaches_mob
WitnessRouteObserved_destroy_runtime_path_destroy_request_reaches_meerkat == <> RouteObserved_destroy_request_reaches_meerkat
WitnessRouteObserved_destroy_runtime_path_runtime_destroyed_reaches_mob == <> RouteObserved_runtime_destroyed_reaches_mob
WitnessRouteObserved_work_terminal_variants_work_failed_reaches_mob == <> RouteObserved_work_failed_reaches_mob
WitnessRouteObserved_work_terminal_variants_work_cancelled_reaches_mob == <> RouteObserved_work_cancelled_reaches_mob

THEOREM Spec => []meerkat_running_has_active_work
THEOREM Spec => []meerkat_bound_runtime_has_fence
THEOREM Spec => []meerkat_destroyed_has_no_active_work
THEOREM Spec => []meerkat_interrupt_pending_only_while_active
THEOREM Spec => []meerkat_drain_requires_ingress_context
THEOREM Spec => []meerkat_peer_reachability_keys_are_resolved
THEOREM Spec => []meerkat_peer_last_reason_keys_are_resolved
THEOREM Spec => []meerkat_active_visibility_revision_not_ahead_of_staged
THEOREM Spec => []meerkat_active_requested_names_subset_of_staged
THEOREM Spec => []meerkat_equal_visibility_revision_means_equal_active_and_staged_state
THEOREM Spec => []meerkat_committed_visibility_not_ahead_of_active
THEOREM Spec => []mob_active_work_requires_runtime
THEOREM Spec => []mob_destroyed_has_no_active_runtime
THEOREM Spec => []mob_active_runtime_has_identity
THEOREM Spec => []mob_active_frames_require_runs
THEOREM Spec => []mob_active_loops_require_frames
THEOREM Spec => []mob_retiring_members_do_not_exceed_active_members
THEOREM Spec => []mob_kickoff_pending_requires_members

=============================================================================
