---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for meerkat_mob_seam.

CONSTANTS AgentIdentityValues, AgentRuntimeIdValues, BooleanValues, FenceTokenValues, GenerationValues, NatValues, PeerReachabilityReasonValues, PeerReachabilityValues, ReachabilityKeyValues, SessionIdValues, SetOfReachabilityKeyValues, SetOfStringValues, StringValues, ToolFilterValues, ToolVisibilityWitnessValues, WorkIdValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionPeerReachabilityReasonValues == {None} \cup {Some(x) : x \in PeerReachabilityReasonValues}
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
      [] route_name = "retire_request_reaches_meerkat" -> "RetireRuntime"
      [] route_name = "destroy_request_reaches_meerkat" -> "DestroyRuntime"
      [] route_name = "runtime_bound_reaches_mob" -> "ObserveRuntimeReady"
      [] route_name = "runtime_retired_reaches_mob" -> "ObserveRuntimeRetired"
      [] route_name = "runtime_destroyed_reaches_mob" -> "ObserveRuntimeDestroyed"
      [] route_name = "work_completed_reaches_mob" -> "ObserveWorkCompleted"
      [] route_name = "work_failed_reaches_mob" -> "ObserveWorkFailed"
      [] route_name = "work_cancelled_reaches_mob" -> "ObserveWorkCancelled"
      [] OTHER -> "unknown_input"

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

VARIABLES meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_active_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_active_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_staged_filter, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_staged_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "mob", variant |-> "ObserveWorkCancelled", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None), work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "work_cancelled_reaches_mob", source_machine |-> "meerkat", source_effect |-> "WorkCancelled", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "ObserveWorkCancelled", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None), work_id |-> packet.payload.work_id], source_kind |-> "route", source_route |-> "work_cancelled_reaches_mob", source_machine |-> "meerkat", source_effect |-> "WorkCancelled", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "work_cancelled_reaches_mob", source_machine |-> "meerkat", effect |-> "WorkCancelled", target_machine |-> "mob", target_input |-> "ObserveWorkCancelled", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None), work_id |-> packet.payload.work_id], actor |-> "mob_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "RunCancelled"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "WorkCancelled", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None), generation |-> (IF "value" \in DOMAIN meerkat_active_generation THEN meerkat_active_generation["value"] ELSE None), work_id |-> packet.payload.work_id], effect_id |-> (model_step_count + 1), source_transition |-> "RunCancelled"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RunCancelled", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Attached"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RecoverRuntime ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RecoverRuntime"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Idle" \/ meerkat_phase = "Stopped" \/ meerkat_phase = "Retired"
       /\ meerkat_phase' = "Recovering"
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeNotice", payload |-> [detail |-> "runtime recovering", kind |-> "recover"], effect_id |-> (model_step_count + 1), source_transition |-> "RecoverRuntime"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RecoverRuntime", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Recovering"]}
       /\ model_step_count' = model_step_count + 1


meerkat_RetireRequestedFromIdle ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "RetireRuntime"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached" \/ meerkat_phase = "Running"
       /\ meerkat_phase' = "Retired"
       /\ meerkat_active_work_id' = None
       /\ meerkat_interrupt_pending' = FALSE
       /\ meerkat_shutdown_pending' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "mob", variant |-> "ObserveRuntimeRetired", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_retired_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeRetired", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "ObserveRuntimeRetired", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_retired_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeRetired", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "runtime_retired_reaches_mob", source_machine |-> "meerkat", effect |-> "RuntimeRetired", target_machine |-> "mob", target_input |-> "ObserveRuntimeRetired", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None)], actor |-> "mob_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "RetireRequestedFromIdle"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeRetired", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None), generation |-> (IF "value" \in DOMAIN meerkat_active_generation THEN meerkat_active_generation["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "RetireRequestedFromIdle"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "RetireRequestedFromIdle", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Retired"]}
       /\ model_step_count' = model_step_count + 1


meerkat_ResetRuntime ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "ResetRuntime"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached" \/ meerkat_phase = "Retired" \/ meerkat_phase = "Stopped" \/ meerkat_phase = "Recovering"
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
       /\ UNCHANGED << meerkat_session_id, meerkat_peer_ingress_configured, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeNotice", payload |-> [detail |-> "runtime reset", kind |-> "reset"], effect_id |-> (model_step_count + 1), source_transition |-> "ResetRuntime"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "ResetRuntime", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Idle"]}
       /\ model_step_count' = model_step_count + 1


meerkat_StopRuntimeExecutor ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "StopRuntimeExecutor"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached" \/ meerkat_phase = "Retired" \/ meerkat_phase = "Recovering"
       /\ meerkat_phase' = "Stopped"
       /\ meerkat_active_work_id' = None
       /\ meerkat_drain_running' = FALSE
       /\ meerkat_interrupt_pending' = FALSE
       /\ meerkat_shutdown_pending' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeNotice", payload |-> [detail |-> "runtime executor stopped", kind |-> "stop"], effect_id |-> (model_step_count + 1), source_transition |-> "StopRuntimeExecutor"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "StopRuntimeExecutor", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Stopped"]}
       /\ model_step_count' = model_step_count + 1


meerkat_DestroyRuntime ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "meerkat"
       /\ packet.variant = "DestroyRuntime"
       /\ ~HigherPriorityReady("meerkat_kernel")
       /\ meerkat_phase = "Attached" \/ meerkat_phase = "Running" \/ meerkat_phase = "Recovering" \/ meerkat_phase = "Retired" \/ meerkat_phase = "Stopped"
       /\ (meerkat_active_runtime_id # None)
       /\ meerkat_phase' = "Destroyed"
       /\ meerkat_active_work_id' = None
       /\ meerkat_wake_pending' = FALSE
       /\ meerkat_process_pending' = FALSE
       /\ meerkat_drain_running' = FALSE
       /\ meerkat_interrupt_pending' = FALSE
       /\ meerkat_shutdown_pending' = FALSE
       /\ UNCHANGED << meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_peer_ingress_configured, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "mob", variant |-> "ObserveRuntimeDestroyed", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_destroyed_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeDestroyed", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "ObserveRuntimeDestroyed", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None)], source_kind |-> "route", source_route |-> "runtime_destroyed_reaches_mob", source_machine |-> "meerkat", source_effect |-> "RuntimeDestroyed", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "runtime_destroyed_reaches_mob", source_machine |-> "meerkat", effect |-> "RuntimeDestroyed", target_machine |-> "mob", target_input |-> "ObserveRuntimeDestroyed", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None)], actor |-> "mob_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "DestroyRuntime"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "meerkat", variant |-> "RuntimeDestroyed", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN meerkat_active_runtime_id THEN meerkat_active_runtime_id["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN meerkat_active_fence_token THEN meerkat_active_fence_token["value"] ELSE None), generation |-> (IF "value" \in DOMAIN meerkat_active_generation THEN meerkat_active_generation["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "DestroyRuntime"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "meerkat", transition |-> "DestroyRuntime", actor |-> "meerkat_kernel", step |-> (model_step_count + 1), from_phase |-> meerkat_phase, to_phase |-> "Destroyed"]}
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "Start", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
       /\ model_step_count' = model_step_count + 1


mob_SpawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "mob"
       /\ packet.variant = "SpawnMember"
       /\ packet.payload.agent_identity = arg_agent_identity
       /\ packet.payload.agent_runtime_id = arg_agent_runtime_id
       /\ packet.payload.fence_token = arg_fence_token
       /\ packet.payload.generation = arg_generation
       /\ ~HigherPriorityReady("mob_kernel")
       /\ mob_phase = "Creating" \/ mob_phase = "Running" \/ mob_phase = "Stopped"
       /\ mob_phase' = "Running"
       /\ mob_active_identity' = Some(packet.payload.agent_identity)
       /\ mob_active_runtime_id' = Some(packet.payload.agent_runtime_id)
       /\ mob_active_fence_token' = Some(packet.payload.fence_token)
       /\ mob_current_generation' = Some(packet.payload.generation)
       /\ mob_active_member_count' = 1
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_inflight_work_id, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "PrepareBindings", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None), generation |-> (IF "value" \in DOMAIN Some(packet.payload.generation) THEN Some(packet.payload.generation)["value"] ELSE None)], source_kind |-> "route", source_route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeBinding", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "PrepareBindings", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None), generation |-> (IF "value" \in DOMAIN Some(packet.payload.generation) THEN Some(packet.payload.generation)["value"] ELSE None)], source_kind |-> "route", source_route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeBinding", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "binding_request_reaches_meerkat", source_machine |-> "mob", effect |-> "RequestRuntimeBinding", target_machine |-> "meerkat", target_input |-> "PrepareBindings", payload |-> [agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None), generation |-> (IF "value" \in DOMAIN Some(packet.payload.generation) THEN Some(packet.payload.generation)["value"] ELSE None)], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "SpawnMember"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "RequestRuntimeBinding", payload |-> [agent_identity |-> (IF "value" \in DOMAIN Some(packet.payload.agent_identity) THEN Some(packet.payload.agent_identity)["value"] ELSE None), agent_runtime_id |-> (IF "value" \in DOMAIN Some(packet.payload.agent_runtime_id) THEN Some(packet.payload.agent_runtime_id)["value"] ELSE None), fence_token |-> (IF "value" \in DOMAIN Some(packet.payload.fence_token) THEN Some(packet.payload.fence_token)["value"] ELSE None), generation |-> (IF "value" \in DOMAIN Some(packet.payload.generation) THEN Some(packet.payload.generation)["value"] ELSE None)], effect_id |-> (model_step_count + 1), source_transition |-> "SpawnMember"], [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [agent_identity |-> (IF "value" \in DOMAIN Some(packet.payload.agent_identity) THEN Some(packet.payload.agent_identity)["value"] ELSE None), kind |-> "spawned"], effect_id |-> (model_step_count + 1), source_transition |-> "SpawnMember"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "SpawnMember", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Running"]}
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_active_member_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "RetireRuntime", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeRetire", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "RetireRuntime", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeRetire", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "retire_request_reaches_meerkat", source_machine |-> "mob", effect |-> "RequestRuntimeRetire", target_machine |-> "meerkat", target_input |-> "RetireRuntime", payload |-> [tag |-> "unit"], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "RetireMember"] }
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_current_generation, mob_active_member_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ mob_phase = "Running" \/ mob_phase = "Stopped"
       /\ mob_phase' = "Running"
       /\ mob_active_identity' = Some(packet.payload.agent_identity)
       /\ mob_active_runtime_id' = Some(packet.payload.agent_runtime_id)
       /\ mob_active_fence_token' = Some(packet.payload.fence_token)
       /\ mob_current_generation' = Some(packet.payload.generation)
       /\ mob_inflight_work_id' = None
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "meerkat", variant |-> "DestroyRuntime", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "destroy_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeDestroy", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "meerkat", variant |-> "DestroyRuntime", payload |-> [tag |-> "unit"], source_kind |-> "route", source_route |-> "destroy_request_reaches_meerkat", source_machine |-> "mob", source_effect |-> "RequestRuntimeDestroy", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "destroy_request_reaches_meerkat", source_machine |-> "mob", effect |-> "RequestRuntimeDestroy", target_machine |-> "meerkat", target_input |-> "DestroyRuntime", payload |-> [tag |-> "unit"], actor |-> "meerkat_kernel", effect_id |-> (model_step_count + 1), source_transition |-> "DestroyMob"] }
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
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "mob", variant |-> "EmitMemberLifecycleNotice", payload |-> [agent_identity |-> (IF "value" \in DOMAIN None THEN None["value"] ELSE None), kind |-> "destroyed"], effect_id |-> (model_step_count + 1), source_transition |-> "ObserveRuntimeDestroyed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "mob", transition |-> "ObserveRuntimeDestroyed", actor |-> "mob_kernel", step |-> (model_step_count + 1), from_phase |-> mob_phase, to_phase |-> "Destroyed"]}
       /\ model_step_count' = model_step_count + 1


mob_active_work_requires_runtime == ((mob_inflight_work_id = None) \/ (mob_active_runtime_id # None))
mob_destroyed_has_no_active_runtime == ((mob_phase # "Destroyed") \/ (mob_active_runtime_id = None))
mob_active_runtime_has_identity == ((mob_active_runtime_id = None) \/ (mob_active_identity # None))

Inject_spawn_member(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation) ==
    /\ ~([machine |-> "mob", variant |-> "SpawnMember", payload |-> [agent_identity |-> arg_agent_identity, agent_runtime_id |-> arg_agent_runtime_id, fence_token |-> arg_fence_token, generation |-> arg_generation], source_kind |-> "entry", source_route |-> "spawn_member", source_machine |-> "external_entry", source_effect |-> "SpawnMember", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "mob", variant |-> "SpawnMember", payload |-> [agent_identity |-> arg_agent_identity, agent_runtime_id |-> arg_agent_runtime_id, fence_token |-> arg_fence_token, generation |-> arg_generation], source_kind |-> "entry", source_route |-> "spawn_member", source_machine |-> "external_entry", source_effect |-> "SpawnMember", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "SpawnMember", payload |-> [agent_identity |-> arg_agent_identity, agent_runtime_id |-> arg_agent_runtime_id, fence_token |-> arg_fence_token, generation |-> arg_generation], source_kind |-> "entry", source_route |-> "spawn_member", source_machine |-> "external_entry", source_effect |-> "SpawnMember", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_submit_work(arg_agent_runtime_id, arg_fence_token, arg_work_id) ==
    /\ ~([machine |-> "mob", variant |-> "SubmitWork", payload |-> [agent_runtime_id |-> arg_agent_runtime_id, fence_token |-> arg_fence_token, work_id |-> arg_work_id], source_kind |-> "entry", source_route |-> "submit_work", source_machine |-> "external_entry", source_effect |-> "SubmitWork", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "mob", variant |-> "SubmitWork", payload |-> [agent_runtime_id |-> arg_agent_runtime_id, fence_token |-> arg_fence_token, work_id |-> arg_work_id], source_kind |-> "entry", source_route |-> "submit_work", source_machine |-> "external_entry", source_effect |-> "SubmitWork", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "SubmitWork", payload |-> [agent_runtime_id |-> arg_agent_runtime_id, fence_token |-> arg_fence_token, work_id |-> arg_work_id], source_kind |-> "entry", source_route |-> "submit_work", source_machine |-> "external_entry", source_effect |-> "SubmitWork", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_retire_member(arg_agent_runtime_id, arg_fence_token) ==
    /\ ~([machine |-> "mob", variant |-> "RetireMember", payload |-> [agent_runtime_id |-> arg_agent_runtime_id, fence_token |-> arg_fence_token], source_kind |-> "entry", source_route |-> "retire_member", source_machine |-> "external_entry", source_effect |-> "RetireMember", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "mob", variant |-> "RetireMember", payload |-> [agent_runtime_id |-> arg_agent_runtime_id, fence_token |-> arg_fence_token], source_kind |-> "entry", source_route |-> "retire_member", source_machine |-> "external_entry", source_effect |-> "RetireMember", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "RetireMember", payload |-> [agent_runtime_id |-> arg_agent_runtime_id, fence_token |-> arg_fence_token], source_kind |-> "entry", source_route |-> "retire_member", source_machine |-> "external_entry", source_effect |-> "RetireMember", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_destroy_mob ==
    /\ ~([machine |-> "mob", variant |-> "DestroyMob", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "destroy_mob", source_machine |-> "external_entry", source_effect |-> "DestroyMob", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "mob", variant |-> "DestroyMob", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "destroy_mob", source_machine |-> "external_entry", source_effect |-> "DestroyMob", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "mob", variant |-> "DestroyMob", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "destroy_mob", source_machine |-> "external_entry", source_effect |-> "DestroyMob", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

DeliverQueuedRoute ==
    /\ Len(pending_routes) > 0
    /\ LET route == Head(pending_routes) IN
       /\ pending_routes' = Tail(pending_routes)
       /\ delivered_routes' = delivered_routes \cup {route}
       /\ model_step_count' = model_step_count + 1
       /\ pending_inputs' = AppendIfMissing(pending_inputs, [machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id]}
       /\ UNCHANGED << meerkat_phase, meerkat_session_id, meerkat_active_runtime_id, meerkat_active_fence_token, meerkat_active_generation, meerkat_active_work_id, meerkat_wake_pending, meerkat_process_pending, meerkat_peer_ingress_configured, meerkat_drain_running, meerkat_resolved_peer_keys, meerkat_peer_reachability, meerkat_peer_last_reason, meerkat_interrupt_pending, meerkat_shutdown_pending, meerkat_inherited_base_filter, meerkat_active_filter, meerkat_staged_filter, meerkat_active_requested_deferred_names, meerkat_staged_requested_deferred_names, meerkat_requested_witnesses, meerkat_filter_witnesses, meerkat_active_visibility_revision, meerkat_staged_visibility_revision, meerkat_committed_visibility_revision, mob_phase, mob_active_identity, mob_active_runtime_id, mob_active_fence_token, mob_current_generation, mob_inflight_work_id, mob_active_member_count, mob_active_run_count, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

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
    \/ meerkat_RecoverRuntime
    \/ meerkat_RetireRequestedFromIdle
    \/ meerkat_ResetRuntime
    \/ meerkat_StopRuntimeExecutor
    \/ meerkat_DestroyRuntime
    \/ mob_Start
    \/ \E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_SpawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)
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
    \/ QuiescentStutter

InjectNext ==
    \/ \E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : Inject_spawn_member(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : Inject_submit_work(arg_agent_runtime_id, arg_fence_token, arg_work_id)
    \/ \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : Inject_retire_member(arg_agent_runtime_id, arg_fence_token)
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
WitnessSpec_basic_round_trip == WitnessInit_basic_round_trip /\ [] [WitnessNext_basic_round_trip]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(meerkat_Initialize) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSession(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSession(arg_session_id)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterAttached(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterRunning(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsAttached(arg_names, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsRunning(arg_names, arg_witnesses)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindings(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextAttached(arg_keep_alive)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextRunning(arg_keep_alive)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedAttached(arg_reason)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedRunning(arg_reason)) /\ WF_vars(\E arg_keys \in SetOfReachabilityKeyValues : \E arg_reachability \in MapReachabilityKeyPeerReachabilityValues : \E arg_last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : meerkat_ReconcileResolvedDirectoryAttached(arg_keys, arg_reachability, arg_last_reason)) /\ WF_vars(\E arg_keys \in SetOfReachabilityKeyValues : \E arg_reachability \in MapReachabilityKeyPeerReachabilityValues : \E arg_last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : meerkat_ReconcileResolvedDirectoryRunning(arg_keys, arg_reachability, arg_last_reason)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : meerkat_RecordSendSucceededAttached(arg_key)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : meerkat_RecordSendSucceededRunning(arg_key)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : \E arg_reason \in PeerReachabilityReasonValues : meerkat_RecordSendFailedAttached(arg_key, arg_reason)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : \E arg_reason \in PeerReachabilityReasonValues : meerkat_RecordSendFailedRunning(arg_key, arg_reason)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : meerkat_BeginRunFromIdle(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(meerkat_InterruptCurrentRun) /\ WF_vars(meerkat_CancelAfterBoundary) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_BoundaryAppliedPromote(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_BoundaryAppliedNoop(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_PublishCommittedVisibleSetAttached(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_PublishCommittedVisibleSetRunning(arg_revision)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunCompleted(arg_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunFailed(arg_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunCancelled(arg_work_id)) /\ WF_vars(meerkat_RecoverRuntime) /\ WF_vars(meerkat_RetireRequestedFromIdle) /\ WF_vars(meerkat_ResetRuntime) /\ WF_vars(meerkat_StopRuntimeExecutor) /\ WF_vars(meerkat_DestroyRuntime) /\ WF_vars(mob_Start) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_SpawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeReady(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_SubmitWork(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkCompleted(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkFailed(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkCancelled(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_RetireMember(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeRetired(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_ResetMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_RespawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(mob_MarkCompleted) /\ WF_vars(mob_DestroyMob) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeDestroyed(arg_agent_runtime_id, arg_fence_token))
WitnessSpec_retire_runtime_path == WitnessInit_retire_runtime_path /\ [] [WitnessNext_retire_runtime_path]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(meerkat_Initialize) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSession(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSession(arg_session_id)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterAttached(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterRunning(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsAttached(arg_names, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsRunning(arg_names, arg_witnesses)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindings(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextAttached(arg_keep_alive)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextRunning(arg_keep_alive)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedAttached(arg_reason)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedRunning(arg_reason)) /\ WF_vars(\E arg_keys \in SetOfReachabilityKeyValues : \E arg_reachability \in MapReachabilityKeyPeerReachabilityValues : \E arg_last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : meerkat_ReconcileResolvedDirectoryAttached(arg_keys, arg_reachability, arg_last_reason)) /\ WF_vars(\E arg_keys \in SetOfReachabilityKeyValues : \E arg_reachability \in MapReachabilityKeyPeerReachabilityValues : \E arg_last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : meerkat_ReconcileResolvedDirectoryRunning(arg_keys, arg_reachability, arg_last_reason)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : meerkat_RecordSendSucceededAttached(arg_key)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : meerkat_RecordSendSucceededRunning(arg_key)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : \E arg_reason \in PeerReachabilityReasonValues : meerkat_RecordSendFailedAttached(arg_key, arg_reason)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : \E arg_reason \in PeerReachabilityReasonValues : meerkat_RecordSendFailedRunning(arg_key, arg_reason)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : meerkat_BeginRunFromIdle(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(meerkat_InterruptCurrentRun) /\ WF_vars(meerkat_CancelAfterBoundary) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_BoundaryAppliedPromote(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_BoundaryAppliedNoop(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_PublishCommittedVisibleSetAttached(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_PublishCommittedVisibleSetRunning(arg_revision)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunCompleted(arg_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunFailed(arg_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunCancelled(arg_work_id)) /\ WF_vars(meerkat_RecoverRuntime) /\ WF_vars(meerkat_RetireRequestedFromIdle) /\ WF_vars(meerkat_ResetRuntime) /\ WF_vars(meerkat_StopRuntimeExecutor) /\ WF_vars(meerkat_DestroyRuntime) /\ WF_vars(mob_Start) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_SpawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeReady(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_SubmitWork(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkCompleted(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkFailed(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkCancelled(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_RetireMember(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeRetired(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_ResetMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_RespawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(mob_MarkCompleted) /\ WF_vars(mob_DestroyMob) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeDestroyed(arg_agent_runtime_id, arg_fence_token))
WitnessSpec_destroy_runtime_path == WitnessInit_destroy_runtime_path /\ [] [WitnessNext_destroy_runtime_path]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(meerkat_Initialize) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSession(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSession(arg_session_id)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterAttached(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterRunning(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsAttached(arg_names, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsRunning(arg_names, arg_witnesses)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindings(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextAttached(arg_keep_alive)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextRunning(arg_keep_alive)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedAttached(arg_reason)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedRunning(arg_reason)) /\ WF_vars(\E arg_keys \in SetOfReachabilityKeyValues : \E arg_reachability \in MapReachabilityKeyPeerReachabilityValues : \E arg_last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : meerkat_ReconcileResolvedDirectoryAttached(arg_keys, arg_reachability, arg_last_reason)) /\ WF_vars(\E arg_keys \in SetOfReachabilityKeyValues : \E arg_reachability \in MapReachabilityKeyPeerReachabilityValues : \E arg_last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : meerkat_ReconcileResolvedDirectoryRunning(arg_keys, arg_reachability, arg_last_reason)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : meerkat_RecordSendSucceededAttached(arg_key)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : meerkat_RecordSendSucceededRunning(arg_key)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : \E arg_reason \in PeerReachabilityReasonValues : meerkat_RecordSendFailedAttached(arg_key, arg_reason)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : \E arg_reason \in PeerReachabilityReasonValues : meerkat_RecordSendFailedRunning(arg_key, arg_reason)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : meerkat_BeginRunFromIdle(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(meerkat_InterruptCurrentRun) /\ WF_vars(meerkat_CancelAfterBoundary) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_BoundaryAppliedPromote(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_BoundaryAppliedNoop(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_PublishCommittedVisibleSetAttached(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_PublishCommittedVisibleSetRunning(arg_revision)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunCompleted(arg_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunFailed(arg_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunCancelled(arg_work_id)) /\ WF_vars(meerkat_RecoverRuntime) /\ WF_vars(meerkat_RetireRequestedFromIdle) /\ WF_vars(meerkat_ResetRuntime) /\ WF_vars(meerkat_StopRuntimeExecutor) /\ WF_vars(meerkat_DestroyRuntime) /\ WF_vars(mob_Start) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_SpawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeReady(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_SubmitWork(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkCompleted(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkFailed(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkCancelled(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_RetireMember(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeRetired(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_ResetMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_RespawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(mob_MarkCompleted) /\ WF_vars(mob_DestroyMob) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeDestroyed(arg_agent_runtime_id, arg_fence_token))
WitnessSpec_work_terminal_variants == WitnessInit_work_terminal_variants /\ [] [WitnessNext_work_terminal_variants]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(meerkat_Initialize) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_RegisterSession(arg_session_id)) /\ WF_vars(\E arg_session_id \in SessionIdValues : meerkat_UnregisterSession(arg_session_id)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterAttached(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_filter \in ToolFilterValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_StagePersistentFilterRunning(arg_filter, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsAttached(arg_names, arg_witnesses)) /\ WF_vars(\E arg_names \in SetOfStringValues : \E arg_witnesses \in MapStringToolVisibilityWitnessValues : meerkat_RequestDeferredToolsRunning(arg_names, arg_witnesses)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : meerkat_PrepareBindings(arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextAttached(arg_keep_alive)) /\ WF_vars(\E arg_keep_alive \in BOOLEAN : meerkat_SetPeerIngressContextRunning(arg_keep_alive)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedAttached(arg_reason)) /\ WF_vars(\E arg_reason \in StringValues : meerkat_NotifyDrainExitedRunning(arg_reason)) /\ WF_vars(\E arg_keys \in SetOfReachabilityKeyValues : \E arg_reachability \in MapReachabilityKeyPeerReachabilityValues : \E arg_last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : meerkat_ReconcileResolvedDirectoryAttached(arg_keys, arg_reachability, arg_last_reason)) /\ WF_vars(\E arg_keys \in SetOfReachabilityKeyValues : \E arg_reachability \in MapReachabilityKeyPeerReachabilityValues : \E arg_last_reason \in MapReachabilityKeyOptionPeerReachabilityReasonValues : meerkat_ReconcileResolvedDirectoryRunning(arg_keys, arg_reachability, arg_last_reason)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : meerkat_RecordSendSucceededAttached(arg_key)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : meerkat_RecordSendSucceededRunning(arg_key)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : \E arg_reason \in PeerReachabilityReasonValues : meerkat_RecordSendFailedAttached(arg_key, arg_reason)) /\ WF_vars(\E arg_key \in ReachabilityKeyValues : \E arg_reason \in PeerReachabilityReasonValues : meerkat_RecordSendFailedRunning(arg_key, arg_reason)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : meerkat_BeginRunFromIdle(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(meerkat_InterruptCurrentRun) /\ WF_vars(meerkat_CancelAfterBoundary) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_BoundaryAppliedPromote(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_BoundaryAppliedNoop(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_PublishCommittedVisibleSetAttached(arg_revision)) /\ WF_vars(\E arg_revision \in 0..2 : meerkat_PublishCommittedVisibleSetRunning(arg_revision)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunCompleted(arg_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunFailed(arg_work_id)) /\ WF_vars(\E arg_work_id \in WorkIdValues : meerkat_RunCancelled(arg_work_id)) /\ WF_vars(meerkat_RecoverRuntime) /\ WF_vars(meerkat_RetireRequestedFromIdle) /\ WF_vars(meerkat_ResetRuntime) /\ WF_vars(meerkat_StopRuntimeExecutor) /\ WF_vars(meerkat_DestroyRuntime) /\ WF_vars(mob_Start) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_SpawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeReady(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_SubmitWork(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkCompleted(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkFailed(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_work_id \in WorkIdValues : mob_ObserveWorkCancelled(arg_agent_runtime_id, arg_fence_token, arg_work_id)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_RetireMember(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeRetired(arg_agent_runtime_id, arg_fence_token)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_ResetMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(\E arg_agent_identity \in AgentIdentityValues : \E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : \E arg_generation \in GenerationValues : mob_RespawnMember(arg_agent_identity, arg_agent_runtime_id, arg_fence_token, arg_generation)) /\ WF_vars(mob_MarkCompleted) /\ WF_vars(mob_DestroyMob) /\ WF_vars(\E arg_agent_runtime_id \in AgentRuntimeIdValues : \E arg_fence_token \in FenceTokenValues : mob_ObserveRuntimeDestroyed(arg_agent_runtime_id, arg_fence_token))

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

=============================================================================
