---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for schedule_mob_bundle.

CONSTANTS NatValues, OccurrenceFailureClassValues, OccurrenceIdValues, StringValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

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
    <<"occurrence", "OccurrenceLifecycleMachine", "occurrence_authority">>
}

RouteNames == {
}

Actors == {
    "occurrence_authority"
}

ActorPriorities == {
}

SchedulerRules == {
}

ActorOfMachine(machine_id) ==
    CASE machine_id = "occurrence" -> "occurrence_authority"
      [] OTHER -> "unknown_actor"

RouteSource(route_name) ==
    "unknown_machine"

RouteEffect(route_name) ==
    "unknown_effect"

RouteTargetMachine(route_name) ==
    "unknown_machine"

RouteTargetInput(route_name) ==
    "unknown_input"

RouteDeliveryKind(route_name) ==
    "Unknown"

RouteTargetActor(route_name) == ActorOfMachine(RouteTargetMachine(route_name))

VARIABLES occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_lease_held, occurrence_lease_owner, occurrence_lease_expiry_utc_ms, occurrence_delivery_correlation_id, occurrence_open_delivery_protocol, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_attempt_count, occurrence_superseded_by_revision, obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_lease_held, occurrence_lease_owner, occurrence_lease_expiry_utc_ms, occurrence_delivery_correlation_id, occurrence_open_delivery_protocol, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_attempt_count, occurrence_superseded_by_revision, obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

RoutePackets == SeqElements(pending_routes) \cup delivered_routes
PendingActors == {ActorOfMachine(packet.machine) : packet \in SeqElements(pending_inputs)}
HigherPriorityReady(actor) == \E priority \in ActorPriorities : /\ priority[2] = actor /\ priority[1] \in PendingActors

BaseInit ==
    /\ occurrence_phase = "Pending"
    /\ occurrence_occurrence_id = "occurrence-0"
    /\ occurrence_schedule_id = "schedule-0"
    /\ occurrence_schedule_revision = 1
    /\ occurrence_occurrence_ordinal = 0
    /\ occurrence_target_binding_key = "target-0"
    /\ occurrence_due_at_utc_ms = 1
    /\ occurrence_lease_held = FALSE
    /\ occurrence_lease_owner = ""
    /\ occurrence_lease_expiry_utc_ms = 0
    /\ occurrence_delivery_correlation_id = None
    /\ occurrence_open_delivery_protocol = None
    /\ occurrence_last_receipt_stage = Some("Planned")
    /\ occurrence_failure_class = None
    /\ occurrence_attempt_count = 0
    /\ occurrence_superseded_by_revision = None
    /\ obligation_occurrence_runtime_delivery = {}
    /\ obligation_occurrence_mob_delivery = {}
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

WitnessInit_mob_delivery_completion_path ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "occurrence", variant |-> "Claim", payload |-> [claim_time_utc_ms |-> 10, lease_expiry_utc_ms |-> 20, owner_id |-> "scheduler-owner"], source_kind |-> "entry", source_route |-> "witness:mob_delivery_completion_path:1", source_machine |-> "external_entry", source_effect |-> "Claim", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "occurrence", variant |-> "Claim", payload |-> [claim_time_utc_ms |-> 10, lease_expiry_utc_ms |-> 20, owner_id |-> "scheduler-owner"], source_kind |-> "entry", source_route |-> "witness:mob_delivery_completion_path:1", source_machine |-> "external_entry", source_effect |-> "Claim", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "occurrence", variant |-> "Claim", payload |-> [claim_time_utc_ms |-> 10, lease_expiry_utc_ms |-> 20, owner_id |-> "scheduler-owner"], source_kind |-> "entry", source_route |-> "witness:mob_delivery_completion_path:1", source_machine |-> "external_entry", source_effect |-> "Claim", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "occurrence", variant |-> "StartMobDispatch", payload |-> [delivery_correlation_id |-> "mob-corr-1"], source_kind |-> "entry", source_route |-> "witness:mob_delivery_completion_path:2", source_machine |-> "external_entry", source_effect |-> "StartMobDispatch", effect_id |-> 0], [machine |-> "occurrence", variant |-> "MobAccepted", payload |-> [attempt_count |-> 1, occurrence_id |-> "occurrence-0"], source_kind |-> "entry", source_route |-> "witness:mob_delivery_completion_path:3", source_machine |-> "external_entry", source_effect |-> "MobAccepted", effect_id |-> 0], [machine |-> "occurrence", variant |-> "MobCompleted", payload |-> [attempt_count |-> 1, occurrence_id |-> "occurrence-0"], source_kind |-> "entry", source_route |-> "witness:mob_delivery_completion_path:4", source_machine |-> "external_entry", source_effect |-> "MobCompleted", effect_id |-> 0]>>

WitnessInit_mob_delivery_failure_path ==
    /\ BaseInit
    /\ pending_inputs = <<[machine |-> "occurrence", variant |-> "Claim", payload |-> [claim_time_utc_ms |-> 10, lease_expiry_utc_ms |-> 20, owner_id |-> "scheduler-owner"], source_kind |-> "entry", source_route |-> "witness:mob_delivery_failure_path:1", source_machine |-> "external_entry", source_effect |-> "Claim", effect_id |-> 0]>>
    /\ observed_inputs = {[machine |-> "occurrence", variant |-> "Claim", payload |-> [claim_time_utc_ms |-> 10, lease_expiry_utc_ms |-> 20, owner_id |-> "scheduler-owner"], source_kind |-> "entry", source_route |-> "witness:mob_delivery_failure_path:1", source_machine |-> "external_entry", source_effect |-> "Claim", effect_id |-> 0]}
    /\ witness_current_script_input = [machine |-> "occurrence", variant |-> "Claim", payload |-> [claim_time_utc_ms |-> 10, lease_expiry_utc_ms |-> 20, owner_id |-> "scheduler-owner"], source_kind |-> "entry", source_route |-> "witness:mob_delivery_failure_path:1", source_machine |-> "external_entry", source_effect |-> "Claim", effect_id |-> 0]
    /\ witness_remaining_script_inputs = <<[machine |-> "occurrence", variant |-> "StartMobDispatch", payload |-> [delivery_correlation_id |-> "mob-corr-2"], source_kind |-> "entry", source_route |-> "witness:mob_delivery_failure_path:2", source_machine |-> "external_entry", source_effect |-> "StartMobDispatch", effect_id |-> 0], [machine |-> "occurrence", variant |-> "MobDeliveryFailed", payload |-> [attempt_count |-> 1, failure_class |-> "TargetMaterializationFailed", occurrence_id |-> "occurrence-0"], source_kind |-> "entry", source_route |-> "witness:mob_delivery_failure_path:3", source_machine |-> "external_entry", source_effect |-> "MobDeliveryFailed", effect_id |-> 0]>>

occurrence__phase_is_terminal(arg_phase) == ((arg_phase = "Completed") \/ (arg_phase = "Skipped") \/ (arg_phase = "Misfired") \/ (arg_phase = "Superseded") \/ (arg_phase = "DeliveryFailed"))

occurrence__claimable_at(store_now_utc_ms) == ((occurrence_phase = "Pending") /\ (occurrence_due_at_utc_ms <= store_now_utc_ms) /\ (~(occurrence_lease_held) \/ (occurrence_lease_expiry_utc_ms <= store_now_utc_ms)))

occurrence_ClaimPending(arg_claim_time_utc_ms, arg_owner_id, arg_lease_expiry_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "Claim"
       /\ packet.payload.claim_time_utc_ms = arg_claim_time_utc_ms
       /\ packet.payload.owner_id = arg_owner_id
       /\ packet.payload.lease_expiry_utc_ms = arg_lease_expiry_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending"
       /\ occurrence__claimable_at(packet.payload.claim_time_utc_ms)
       /\ occurrence_phase' = "Claimed"
       /\ occurrence_lease_held' = TRUE
       /\ occurrence_lease_owner' = packet.payload.owner_id
       /\ occurrence_lease_expiry_utc_ms' = packet.payload.lease_expiry_utc_ms
       /\ occurrence_delivery_correlation_id' = None
       /\ occurrence_open_delivery_protocol' = None
       /\ occurrence_last_receipt_stage' = Some("Claimed")
       /\ occurrence_failure_class' = None
       /\ occurrence_attempt_count' = (occurrence_attempt_count) + 1
       /\ occurrence_superseded_by_revision' = None
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "ClaimLeaseGranted", payload |-> [attempt_count |-> (occurrence_attempt_count) + 1, lease_expiry_utc_ms |-> packet.payload.lease_expiry_utc_ms, occurrence_id |-> occurrence_occurrence_id, owner_id |-> packet.payload.owner_id], effect_id |-> (model_step_count + 1), source_transition |-> "ClaimPending"], [machine |-> "occurrence", variant |-> "AppendReceipt", payload |-> [stage |-> "Claimed"], effect_id |-> (model_step_count + 1), source_transition |-> "ClaimPending"], [machine |-> "occurrence", variant |-> "EmitOccurrenceNotice", payload |-> [new_state |-> occurrence_phase], effect_id |-> (model_step_count + 1), source_transition |-> "ClaimPending"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClaimPending", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Claimed"]}
       /\ UNCHANGED << obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery >>
       /\ model_step_count' = model_step_count + 1


occurrence_StartRuntimeDispatchFromClaimed(arg_delivery_correlation_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "StartRuntimeDispatch"
       /\ packet.payload.delivery_correlation_id = arg_delivery_correlation_id
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed"
       /\ occurrence_phase' = "Dispatching"
       /\ occurrence_delivery_correlation_id' = Some(packet.payload.delivery_correlation_id)
       /\ occurrence_open_delivery_protocol' = Some("Runtime")
       /\ occurrence_last_receipt_stage' = Some("DispatchStarted")
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_lease_held, occurrence_lease_owner, occurrence_lease_expiry_utc_ms, occurrence_failure_class, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DispatchToRuntime", payload |-> [attempt_count |-> occurrence_attempt_count, delivery_correlation_id |-> Some(packet.payload.delivery_correlation_id), occurrence_id |-> occurrence_occurrence_id, occurrence_ordinal |-> occurrence_occurrence_ordinal, schedule_id |-> occurrence_schedule_id, schedule_revision |-> occurrence_schedule_revision, target_binding_key |-> occurrence_target_binding_key], effect_id |-> (model_step_count + 1), source_transition |-> "StartRuntimeDispatchFromClaimed"], [machine |-> "occurrence", variant |-> "AppendReceipt", payload |-> [stage |-> "DispatchStarted"], effect_id |-> (model_step_count + 1), source_transition |-> "StartRuntimeDispatchFromClaimed"], [machine |-> "occurrence", variant |-> "EmitOccurrenceNotice", payload |-> [new_state |-> occurrence_phase], effect_id |-> (model_step_count + 1), source_transition |-> "StartRuntimeDispatchFromClaimed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "StartRuntimeDispatchFromClaimed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Dispatching"]}
       /\ obligation_occurrence_runtime_delivery' = obligation_occurrence_runtime_delivery \cup {[occurrence_id |-> occurrence_occurrence_id, schedule_id |-> occurrence_schedule_id, schedule_revision |-> occurrence_schedule_revision, occurrence_ordinal |-> occurrence_occurrence_ordinal, attempt_count |-> occurrence_attempt_count, target_binding_key |-> occurrence_target_binding_key, delivery_correlation_id |-> Some(packet.payload.delivery_correlation_id)]}
       /\ UNCHANGED << obligation_occurrence_mob_delivery >>
       /\ model_step_count' = model_step_count + 1


occurrence_StartMobDispatchFromClaimed(arg_delivery_correlation_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "StartMobDispatch"
       /\ packet.payload.delivery_correlation_id = arg_delivery_correlation_id
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed"
       /\ occurrence_phase' = "Dispatching"
       /\ occurrence_delivery_correlation_id' = Some(packet.payload.delivery_correlation_id)
       /\ occurrence_open_delivery_protocol' = Some("Mob")
       /\ occurrence_last_receipt_stage' = Some("DispatchStarted")
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_lease_held, occurrence_lease_owner, occurrence_lease_expiry_utc_ms, occurrence_failure_class, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DispatchToMob", payload |-> [attempt_count |-> occurrence_attempt_count, delivery_correlation_id |-> Some(packet.payload.delivery_correlation_id), occurrence_id |-> occurrence_occurrence_id, occurrence_ordinal |-> occurrence_occurrence_ordinal, schedule_id |-> occurrence_schedule_id, schedule_revision |-> occurrence_schedule_revision, target_binding_key |-> occurrence_target_binding_key], effect_id |-> (model_step_count + 1), source_transition |-> "StartMobDispatchFromClaimed"], [machine |-> "occurrence", variant |-> "AppendReceipt", payload |-> [stage |-> "DispatchStarted"], effect_id |-> (model_step_count + 1), source_transition |-> "StartMobDispatchFromClaimed"], [machine |-> "occurrence", variant |-> "EmitOccurrenceNotice", payload |-> [new_state |-> occurrence_phase], effect_id |-> (model_step_count + 1), source_transition |-> "StartMobDispatchFromClaimed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "StartMobDispatchFromClaimed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Dispatching"]}
       /\ obligation_occurrence_mob_delivery' = obligation_occurrence_mob_delivery \cup {[occurrence_id |-> occurrence_occurrence_id, schedule_id |-> occurrence_schedule_id, schedule_revision |-> occurrence_schedule_revision, occurrence_ordinal |-> occurrence_occurrence_ordinal, attempt_count |-> occurrence_attempt_count, target_binding_key |-> occurrence_target_binding_key, delivery_correlation_id |-> Some(packet.payload.delivery_correlation_id)]}
       /\ UNCHANGED << obligation_occurrence_runtime_delivery >>
       /\ model_step_count' = model_step_count + 1


occurrence_RuntimeAcceptedFromDispatching(arg_occurrence_id, arg_attempt_count) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "RuntimeAccepted"
       /\ packet.payload.occurrence_id = arg_occurrence_id
       /\ packet.payload.attempt_count = arg_attempt_count
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching"
       /\ (occurrence_open_delivery_protocol = Some("Runtime"))
       /\ (occurrence_occurrence_id = packet.payload.occurrence_id)
       /\ (occurrence_attempt_count = packet.payload.attempt_count)
       /\ occurrence_phase' = "AwaitingCompletion"
       /\ occurrence_last_receipt_stage' = Some("AwaitingCompletion")
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_lease_held, occurrence_lease_owner, occurrence_lease_expiry_utc_ms, occurrence_delivery_correlation_id, occurrence_open_delivery_protocol, occurrence_failure_class, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "AppendReceipt", payload |-> [stage |-> "AwaitingCompletion"], effect_id |-> (model_step_count + 1), source_transition |-> "RuntimeAcceptedFromDispatching"], [machine |-> "occurrence", variant |-> "EmitOccurrenceNotice", payload |-> [new_state |-> occurrence_phase], effect_id |-> (model_step_count + 1), source_transition |-> "RuntimeAcceptedFromDispatching"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "RuntimeAcceptedFromDispatching", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "AwaitingCompletion"]}
       /\ UNCHANGED << obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery >>
       /\ model_step_count' = model_step_count + 1


occurrence_MobAcceptedFromDispatching(arg_occurrence_id, arg_attempt_count) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "MobAccepted"
       /\ packet.payload.occurrence_id = arg_occurrence_id
       /\ packet.payload.attempt_count = arg_attempt_count
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching"
       /\ (occurrence_open_delivery_protocol = Some("Mob"))
       /\ (occurrence_occurrence_id = packet.payload.occurrence_id)
       /\ (occurrence_attempt_count = packet.payload.attempt_count)
       /\ occurrence_phase' = "AwaitingCompletion"
       /\ occurrence_last_receipt_stage' = Some("AwaitingCompletion")
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_lease_held, occurrence_lease_owner, occurrence_lease_expiry_utc_ms, occurrence_delivery_correlation_id, occurrence_open_delivery_protocol, occurrence_failure_class, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "AppendReceipt", payload |-> [stage |-> "AwaitingCompletion"], effect_id |-> (model_step_count + 1), source_transition |-> "MobAcceptedFromDispatching"], [machine |-> "occurrence", variant |-> "EmitOccurrenceNotice", payload |-> [new_state |-> occurrence_phase], effect_id |-> (model_step_count + 1), source_transition |-> "MobAcceptedFromDispatching"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "MobAcceptedFromDispatching", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "AwaitingCompletion"]}
       /\ UNCHANGED << obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery >>
       /\ model_step_count' = model_step_count + 1


occurrence_RuntimeCompletedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "RuntimeCompleted"
       /\ packet.payload.occurrence_id = arg_occurrence_id
       /\ packet.payload.attempt_count = arg_attempt_count
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ (occurrence_open_delivery_protocol = Some("Runtime"))
       /\ (occurrence_occurrence_id = packet.payload.occurrence_id)
       /\ (occurrence_attempt_count = packet.payload.attempt_count)
       /\ occurrence_phase' = "Completed"
       /\ occurrence_lease_held' = FALSE
       /\ occurrence_lease_owner' = ""
       /\ occurrence_lease_expiry_utc_ms' = 0
       /\ occurrence_open_delivery_protocol' = None
       /\ occurrence_last_receipt_stage' = Some("Completed")
       /\ occurrence_failure_class' = None
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_delivery_correlation_id, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "AppendReceipt", payload |-> [stage |-> "Completed"], effect_id |-> (model_step_count + 1), source_transition |-> "RuntimeCompletedFromDispatchingOrAwaiting"], [machine |-> "occurrence", variant |-> "EmitOccurrenceNotice", payload |-> [new_state |-> occurrence_phase], effect_id |-> (model_step_count + 1), source_transition |-> "RuntimeCompletedFromDispatchingOrAwaiting"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "RuntimeCompletedFromDispatchingOrAwaiting", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Completed"]}
       /\ UNCHANGED << obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery >>
       /\ model_step_count' = model_step_count + 1


occurrence_MobCompletedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "MobCompleted"
       /\ packet.payload.occurrence_id = arg_occurrence_id
       /\ packet.payload.attempt_count = arg_attempt_count
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ (occurrence_open_delivery_protocol = Some("Mob"))
       /\ (occurrence_occurrence_id = packet.payload.occurrence_id)
       /\ (occurrence_attempt_count = packet.payload.attempt_count)
       /\ occurrence_phase' = "Completed"
       /\ occurrence_lease_held' = FALSE
       /\ occurrence_lease_owner' = ""
       /\ occurrence_lease_expiry_utc_ms' = 0
       /\ occurrence_open_delivery_protocol' = None
       /\ occurrence_last_receipt_stage' = Some("Completed")
       /\ occurrence_failure_class' = None
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_delivery_correlation_id, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "AppendReceipt", payload |-> [stage |-> "Completed"], effect_id |-> (model_step_count + 1), source_transition |-> "MobCompletedFromDispatchingOrAwaiting"], [machine |-> "occurrence", variant |-> "EmitOccurrenceNotice", payload |-> [new_state |-> occurrence_phase], effect_id |-> (model_step_count + 1), source_transition |-> "MobCompletedFromDispatchingOrAwaiting"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "MobCompletedFromDispatchingOrAwaiting", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Completed"]}
       /\ UNCHANGED << obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery >>
       /\ model_step_count' = model_step_count + 1


occurrence_RuntimeSkippedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "RuntimeSkipped"
       /\ packet.payload.occurrence_id = arg_occurrence_id
       /\ packet.payload.attempt_count = arg_attempt_count
       /\ packet.payload.failure_class = arg_failure_class
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ (occurrence_open_delivery_protocol = Some("Runtime"))
       /\ (occurrence_occurrence_id = packet.payload.occurrence_id)
       /\ (occurrence_attempt_count = packet.payload.attempt_count)
       /\ occurrence_phase' = "Skipped"
       /\ occurrence_lease_held' = FALSE
       /\ occurrence_lease_owner' = ""
       /\ occurrence_lease_expiry_utc_ms' = 0
       /\ occurrence_open_delivery_protocol' = None
       /\ occurrence_last_receipt_stage' = Some("Skipped")
       /\ occurrence_failure_class' = Some(packet.payload.failure_class)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_delivery_correlation_id, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "AppendReceipt", payload |-> [stage |-> "Skipped"], effect_id |-> (model_step_count + 1), source_transition |-> "RuntimeSkippedFromDispatchingOrAwaiting"], [machine |-> "occurrence", variant |-> "EmitOccurrenceNotice", payload |-> [new_state |-> occurrence_phase], effect_id |-> (model_step_count + 1), source_transition |-> "RuntimeSkippedFromDispatchingOrAwaiting"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "RuntimeSkippedFromDispatchingOrAwaiting", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Skipped"]}
       /\ UNCHANGED << obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery >>
       /\ model_step_count' = model_step_count + 1


occurrence_MobSkippedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "MobSkipped"
       /\ packet.payload.occurrence_id = arg_occurrence_id
       /\ packet.payload.attempt_count = arg_attempt_count
       /\ packet.payload.failure_class = arg_failure_class
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ (occurrence_open_delivery_protocol = Some("Mob"))
       /\ (occurrence_occurrence_id = packet.payload.occurrence_id)
       /\ (occurrence_attempt_count = packet.payload.attempt_count)
       /\ occurrence_phase' = "Skipped"
       /\ occurrence_lease_held' = FALSE
       /\ occurrence_lease_owner' = ""
       /\ occurrence_lease_expiry_utc_ms' = 0
       /\ occurrence_open_delivery_protocol' = None
       /\ occurrence_last_receipt_stage' = Some("Skipped")
       /\ occurrence_failure_class' = Some(packet.payload.failure_class)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_delivery_correlation_id, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "AppendReceipt", payload |-> [stage |-> "Skipped"], effect_id |-> (model_step_count + 1), source_transition |-> "MobSkippedFromDispatchingOrAwaiting"], [machine |-> "occurrence", variant |-> "EmitOccurrenceNotice", payload |-> [new_state |-> occurrence_phase], effect_id |-> (model_step_count + 1), source_transition |-> "MobSkippedFromDispatchingOrAwaiting"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "MobSkippedFromDispatchingOrAwaiting", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Skipped"]}
       /\ UNCHANGED << obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery >>
       /\ model_step_count' = model_step_count + 1


occurrence_RuntimeMisfiredFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "RuntimeMisfired"
       /\ packet.payload.occurrence_id = arg_occurrence_id
       /\ packet.payload.attempt_count = arg_attempt_count
       /\ packet.payload.failure_class = arg_failure_class
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ (occurrence_open_delivery_protocol = Some("Runtime"))
       /\ (occurrence_occurrence_id = packet.payload.occurrence_id)
       /\ (occurrence_attempt_count = packet.payload.attempt_count)
       /\ occurrence_phase' = "Misfired"
       /\ occurrence_lease_held' = FALSE
       /\ occurrence_lease_owner' = ""
       /\ occurrence_lease_expiry_utc_ms' = 0
       /\ occurrence_open_delivery_protocol' = None
       /\ occurrence_last_receipt_stage' = Some("Misfired")
       /\ occurrence_failure_class' = Some(packet.payload.failure_class)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_delivery_correlation_id, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "AppendReceipt", payload |-> [stage |-> "Misfired"], effect_id |-> (model_step_count + 1), source_transition |-> "RuntimeMisfiredFromDispatchingOrAwaiting"], [machine |-> "occurrence", variant |-> "EmitOccurrenceNotice", payload |-> [new_state |-> occurrence_phase], effect_id |-> (model_step_count + 1), source_transition |-> "RuntimeMisfiredFromDispatchingOrAwaiting"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "RuntimeMisfiredFromDispatchingOrAwaiting", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Misfired"]}
       /\ UNCHANGED << obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery >>
       /\ model_step_count' = model_step_count + 1


occurrence_MobMisfiredFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "MobMisfired"
       /\ packet.payload.occurrence_id = arg_occurrence_id
       /\ packet.payload.attempt_count = arg_attempt_count
       /\ packet.payload.failure_class = arg_failure_class
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ (occurrence_open_delivery_protocol = Some("Mob"))
       /\ (occurrence_occurrence_id = packet.payload.occurrence_id)
       /\ (occurrence_attempt_count = packet.payload.attempt_count)
       /\ occurrence_phase' = "Misfired"
       /\ occurrence_lease_held' = FALSE
       /\ occurrence_lease_owner' = ""
       /\ occurrence_lease_expiry_utc_ms' = 0
       /\ occurrence_open_delivery_protocol' = None
       /\ occurrence_last_receipt_stage' = Some("Misfired")
       /\ occurrence_failure_class' = Some(packet.payload.failure_class)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_delivery_correlation_id, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "AppendReceipt", payload |-> [stage |-> "Misfired"], effect_id |-> (model_step_count + 1), source_transition |-> "MobMisfiredFromDispatchingOrAwaiting"], [machine |-> "occurrence", variant |-> "EmitOccurrenceNotice", payload |-> [new_state |-> occurrence_phase], effect_id |-> (model_step_count + 1), source_transition |-> "MobMisfiredFromDispatchingOrAwaiting"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "MobMisfiredFromDispatchingOrAwaiting", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Misfired"]}
       /\ UNCHANGED << obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery >>
       /\ model_step_count' = model_step_count + 1


occurrence_RuntimeDeliveryFailedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "RuntimeDeliveryFailed"
       /\ packet.payload.occurrence_id = arg_occurrence_id
       /\ packet.payload.attempt_count = arg_attempt_count
       /\ packet.payload.failure_class = arg_failure_class
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ (occurrence_open_delivery_protocol = Some("Runtime"))
       /\ (occurrence_occurrence_id = packet.payload.occurrence_id)
       /\ (occurrence_attempt_count = packet.payload.attempt_count)
       /\ occurrence_phase' = "DeliveryFailed"
       /\ occurrence_lease_held' = FALSE
       /\ occurrence_lease_owner' = ""
       /\ occurrence_lease_expiry_utc_ms' = 0
       /\ occurrence_open_delivery_protocol' = None
       /\ occurrence_last_receipt_stage' = Some("DeliveryFailed")
       /\ occurrence_failure_class' = Some(packet.payload.failure_class)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_delivery_correlation_id, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "AppendReceipt", payload |-> [stage |-> "DeliveryFailed"], effect_id |-> (model_step_count + 1), source_transition |-> "RuntimeDeliveryFailedFromDispatchingOrAwaiting"], [machine |-> "occurrence", variant |-> "EmitOccurrenceNotice", payload |-> [new_state |-> occurrence_phase], effect_id |-> (model_step_count + 1), source_transition |-> "RuntimeDeliveryFailedFromDispatchingOrAwaiting"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "RuntimeDeliveryFailedFromDispatchingOrAwaiting", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "DeliveryFailed"]}
       /\ UNCHANGED << obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery >>
       /\ model_step_count' = model_step_count + 1


occurrence_MobDeliveryFailedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "MobDeliveryFailed"
       /\ packet.payload.occurrence_id = arg_occurrence_id
       /\ packet.payload.attempt_count = arg_attempt_count
       /\ packet.payload.failure_class = arg_failure_class
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ (occurrence_open_delivery_protocol = Some("Mob"))
       /\ (occurrence_occurrence_id = packet.payload.occurrence_id)
       /\ (occurrence_attempt_count = packet.payload.attempt_count)
       /\ occurrence_phase' = "DeliveryFailed"
       /\ occurrence_lease_held' = FALSE
       /\ occurrence_lease_owner' = ""
       /\ occurrence_lease_expiry_utc_ms' = 0
       /\ occurrence_open_delivery_protocol' = None
       /\ occurrence_last_receipt_stage' = Some("DeliveryFailed")
       /\ occurrence_failure_class' = Some(packet.payload.failure_class)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_delivery_correlation_id, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "AppendReceipt", payload |-> [stage |-> "DeliveryFailed"], effect_id |-> (model_step_count + 1), source_transition |-> "MobDeliveryFailedFromDispatchingOrAwaiting"], [machine |-> "occurrence", variant |-> "EmitOccurrenceNotice", payload |-> [new_state |-> occurrence_phase], effect_id |-> (model_step_count + 1), source_transition |-> "MobDeliveryFailedFromDispatchingOrAwaiting"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "MobDeliveryFailedFromDispatchingOrAwaiting", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "DeliveryFailed"]}
       /\ UNCHANGED << obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery >>
       /\ model_step_count' = model_step_count + 1


occurrence_SupersedePending(arg_superseding_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "SupersedeByRevision"
       /\ packet.payload.superseding_revision = arg_superseding_revision
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending"
       /\ (packet.payload.superseding_revision > occurrence_schedule_revision)
       /\ occurrence_phase' = "Superseded"
       /\ occurrence_open_delivery_protocol' = None
       /\ occurrence_last_receipt_stage' = Some("Superseded")
       /\ occurrence_superseded_by_revision' = Some(packet.payload.superseding_revision)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_lease_held, occurrence_lease_owner, occurrence_lease_expiry_utc_ms, occurrence_delivery_correlation_id, occurrence_failure_class, occurrence_attempt_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "AppendReceipt", payload |-> [stage |-> "Superseded"], effect_id |-> (model_step_count + 1), source_transition |-> "SupersedePending"], [machine |-> "occurrence", variant |-> "EmitOccurrenceNotice", payload |-> [new_state |-> occurrence_phase], effect_id |-> (model_step_count + 1), source_transition |-> "SupersedePending"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "SupersedePending", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Superseded"]}
       /\ UNCHANGED << obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery >>
       /\ model_step_count' = model_step_count + 1


occurrence_LeaseExpiredFromClaimed ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "LeaseExpired"
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed"
       /\ (occurrence_lease_held = TRUE)
       /\ occurrence_phase' = "Pending"
       /\ occurrence_lease_held' = FALSE
       /\ occurrence_lease_owner' = ""
       /\ occurrence_lease_expiry_utc_ms' = 0
       /\ occurrence_delivery_correlation_id' = None
       /\ occurrence_open_delivery_protocol' = None
       /\ occurrence_last_receipt_stage' = Some("LeaseExpired")
       /\ occurrence_failure_class' = None
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "AppendReceipt", payload |-> [stage |-> "LeaseExpired"], effect_id |-> (model_step_count + 1), source_transition |-> "LeaseExpiredFromClaimed"], [machine |-> "occurrence", variant |-> "EmitOccurrenceNotice", payload |-> [new_state |-> occurrence_phase], effect_id |-> (model_step_count + 1), source_transition |-> "LeaseExpiredFromClaimed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "LeaseExpiredFromClaimed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ UNCHANGED << obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery >>
       /\ model_step_count' = model_step_count + 1


occurrence_LeaseExpiredFromDispatching ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "LeaseExpired"
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching"
       /\ (occurrence_lease_held = TRUE)
       /\ occurrence_phase' = "Pending"
       /\ occurrence_lease_held' = FALSE
       /\ occurrence_lease_owner' = ""
       /\ occurrence_lease_expiry_utc_ms' = 0
       /\ occurrence_delivery_correlation_id' = None
       /\ occurrence_open_delivery_protocol' = None
       /\ occurrence_last_receipt_stage' = Some("LeaseExpired")
       /\ occurrence_failure_class' = None
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "AppendReceipt", payload |-> [stage |-> "LeaseExpired"], effect_id |-> (model_step_count + 1), source_transition |-> "LeaseExpiredFromDispatching"], [machine |-> "occurrence", variant |-> "EmitOccurrenceNotice", payload |-> [new_state |-> occurrence_phase], effect_id |-> (model_step_count + 1), source_transition |-> "LeaseExpiredFromDispatching"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "LeaseExpiredFromDispatching", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ UNCHANGED << obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery >>
       /\ model_step_count' = model_step_count + 1


occurrence_LeaseExpiredFromAwaitingCompletion ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "LeaseExpired"
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "AwaitingCompletion"
       /\ (occurrence_lease_held = TRUE)
       /\ occurrence_phase' = "Pending"
       /\ occurrence_lease_held' = FALSE
       /\ occurrence_lease_owner' = ""
       /\ occurrence_lease_expiry_utc_ms' = 0
       /\ occurrence_delivery_correlation_id' = None
       /\ occurrence_open_delivery_protocol' = None
       /\ occurrence_last_receipt_stage' = Some("LeaseExpired")
       /\ occurrence_failure_class' = None
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "AppendReceipt", payload |-> [stage |-> "LeaseExpired"], effect_id |-> (model_step_count + 1), source_transition |-> "LeaseExpiredFromAwaitingCompletion"], [machine |-> "occurrence", variant |-> "EmitOccurrenceNotice", payload |-> [new_state |-> occurrence_phase], effect_id |-> (model_step_count + 1), source_transition |-> "LeaseExpiredFromAwaitingCompletion"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "LeaseExpiredFromAwaitingCompletion", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ UNCHANGED << obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery >>
       /\ model_step_count' = model_step_count + 1


occurrence_terminal_has_no_open_delivery_protocol == (~(occurrence__phase_is_terminal(occurrence_phase)) \/ (occurrence_open_delivery_protocol = None))
occurrence_live_claim_requires_lease_holder == (((occurrence_phase # "Claimed") /\ (occurrence_phase # "Dispatching") /\ (occurrence_phase # "AwaitingCompletion")) \/ (occurrence_lease_held = TRUE))
occurrence_awaiting_completion_requires_protocol == ((occurrence_phase # "AwaitingCompletion") \/ (occurrence_open_delivery_protocol # None))
occurrence_superseded_records_revision == ((occurrence_phase # "Superseded") \/ (occurrence_superseded_by_revision # None))
occurrence_delivery_failed_records_failure_class == ((occurrence_phase # "DeliveryFailed") \/ (occurrence_failure_class # None))

Inject_occurrence_claim(arg_claim_time_utc_ms, arg_owner_id, arg_lease_expiry_utc_ms) ==
    /\ ~([machine |-> "occurrence", variant |-> "Claim", payload |-> [claim_time_utc_ms |-> arg_claim_time_utc_ms, lease_expiry_utc_ms |-> arg_lease_expiry_utc_ms, owner_id |-> arg_owner_id], source_kind |-> "entry", source_route |-> "occurrence_claim", source_machine |-> "external_entry", source_effect |-> "Claim", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "occurrence", variant |-> "Claim", payload |-> [claim_time_utc_ms |-> arg_claim_time_utc_ms, lease_expiry_utc_ms |-> arg_lease_expiry_utc_ms, owner_id |-> arg_owner_id], source_kind |-> "entry", source_route |-> "occurrence_claim", source_machine |-> "external_entry", source_effect |-> "Claim", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "occurrence", variant |-> "Claim", payload |-> [claim_time_utc_ms |-> arg_claim_time_utc_ms, lease_expiry_utc_ms |-> arg_lease_expiry_utc_ms, owner_id |-> arg_owner_id], source_kind |-> "entry", source_route |-> "occurrence_claim", source_machine |-> "external_entry", source_effect |-> "Claim", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_lease_held, occurrence_lease_owner, occurrence_lease_expiry_utc_ms, occurrence_delivery_correlation_id, occurrence_open_delivery_protocol, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_attempt_count, occurrence_superseded_by_revision, obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_occurrence_start_mob_dispatch(arg_delivery_correlation_id) ==
    /\ ~([machine |-> "occurrence", variant |-> "StartMobDispatch", payload |-> [delivery_correlation_id |-> arg_delivery_correlation_id], source_kind |-> "entry", source_route |-> "occurrence_start_mob_dispatch", source_machine |-> "external_entry", source_effect |-> "StartMobDispatch", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "occurrence", variant |-> "StartMobDispatch", payload |-> [delivery_correlation_id |-> arg_delivery_correlation_id], source_kind |-> "entry", source_route |-> "occurrence_start_mob_dispatch", source_machine |-> "external_entry", source_effect |-> "StartMobDispatch", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "occurrence", variant |-> "StartMobDispatch", payload |-> [delivery_correlation_id |-> arg_delivery_correlation_id], source_kind |-> "entry", source_route |-> "occurrence_start_mob_dispatch", source_machine |-> "external_entry", source_effect |-> "StartMobDispatch", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_lease_held, occurrence_lease_owner, occurrence_lease_expiry_utc_ms, occurrence_delivery_correlation_id, occurrence_open_delivery_protocol, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_attempt_count, occurrence_superseded_by_revision, obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_occurrence_mob_accepted(arg_occurrence_id, arg_attempt_count) ==
    /\ ~([machine |-> "occurrence", variant |-> "MobAccepted", payload |-> [attempt_count |-> arg_attempt_count, occurrence_id |-> arg_occurrence_id], source_kind |-> "entry", source_route |-> "occurrence_mob_accepted", source_machine |-> "external_entry", source_effect |-> "MobAccepted", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "occurrence", variant |-> "MobAccepted", payload |-> [attempt_count |-> arg_attempt_count, occurrence_id |-> arg_occurrence_id], source_kind |-> "entry", source_route |-> "occurrence_mob_accepted", source_machine |-> "external_entry", source_effect |-> "MobAccepted", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "occurrence", variant |-> "MobAccepted", payload |-> [attempt_count |-> arg_attempt_count, occurrence_id |-> arg_occurrence_id], source_kind |-> "entry", source_route |-> "occurrence_mob_accepted", source_machine |-> "external_entry", source_effect |-> "MobAccepted", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_lease_held, occurrence_lease_owner, occurrence_lease_expiry_utc_ms, occurrence_delivery_correlation_id, occurrence_open_delivery_protocol, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_attempt_count, occurrence_superseded_by_revision, obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_occurrence_mob_completed(arg_occurrence_id, arg_attempt_count) ==
    /\ ~([machine |-> "occurrence", variant |-> "MobCompleted", payload |-> [attempt_count |-> arg_attempt_count, occurrence_id |-> arg_occurrence_id], source_kind |-> "entry", source_route |-> "occurrence_mob_completed", source_machine |-> "external_entry", source_effect |-> "MobCompleted", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "occurrence", variant |-> "MobCompleted", payload |-> [attempt_count |-> arg_attempt_count, occurrence_id |-> arg_occurrence_id], source_kind |-> "entry", source_route |-> "occurrence_mob_completed", source_machine |-> "external_entry", source_effect |-> "MobCompleted", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "occurrence", variant |-> "MobCompleted", payload |-> [attempt_count |-> arg_attempt_count, occurrence_id |-> arg_occurrence_id], source_kind |-> "entry", source_route |-> "occurrence_mob_completed", source_machine |-> "external_entry", source_effect |-> "MobCompleted", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_lease_held, occurrence_lease_owner, occurrence_lease_expiry_utc_ms, occurrence_delivery_correlation_id, occurrence_open_delivery_protocol, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_attempt_count, occurrence_superseded_by_revision, obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_occurrence_mob_skipped(arg_occurrence_id, arg_attempt_count, arg_failure_class) ==
    /\ ~([machine |-> "occurrence", variant |-> "MobSkipped", payload |-> [attempt_count |-> arg_attempt_count, failure_class |-> arg_failure_class, occurrence_id |-> arg_occurrence_id], source_kind |-> "entry", source_route |-> "occurrence_mob_skipped", source_machine |-> "external_entry", source_effect |-> "MobSkipped", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "occurrence", variant |-> "MobSkipped", payload |-> [attempt_count |-> arg_attempt_count, failure_class |-> arg_failure_class, occurrence_id |-> arg_occurrence_id], source_kind |-> "entry", source_route |-> "occurrence_mob_skipped", source_machine |-> "external_entry", source_effect |-> "MobSkipped", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "occurrence", variant |-> "MobSkipped", payload |-> [attempt_count |-> arg_attempt_count, failure_class |-> arg_failure_class, occurrence_id |-> arg_occurrence_id], source_kind |-> "entry", source_route |-> "occurrence_mob_skipped", source_machine |-> "external_entry", source_effect |-> "MobSkipped", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_lease_held, occurrence_lease_owner, occurrence_lease_expiry_utc_ms, occurrence_delivery_correlation_id, occurrence_open_delivery_protocol, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_attempt_count, occurrence_superseded_by_revision, obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_occurrence_mob_misfired(arg_occurrence_id, arg_attempt_count, arg_failure_class) ==
    /\ ~([machine |-> "occurrence", variant |-> "MobMisfired", payload |-> [attempt_count |-> arg_attempt_count, failure_class |-> arg_failure_class, occurrence_id |-> arg_occurrence_id], source_kind |-> "entry", source_route |-> "occurrence_mob_misfired", source_machine |-> "external_entry", source_effect |-> "MobMisfired", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "occurrence", variant |-> "MobMisfired", payload |-> [attempt_count |-> arg_attempt_count, failure_class |-> arg_failure_class, occurrence_id |-> arg_occurrence_id], source_kind |-> "entry", source_route |-> "occurrence_mob_misfired", source_machine |-> "external_entry", source_effect |-> "MobMisfired", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "occurrence", variant |-> "MobMisfired", payload |-> [attempt_count |-> arg_attempt_count, failure_class |-> arg_failure_class, occurrence_id |-> arg_occurrence_id], source_kind |-> "entry", source_route |-> "occurrence_mob_misfired", source_machine |-> "external_entry", source_effect |-> "MobMisfired", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_lease_held, occurrence_lease_owner, occurrence_lease_expiry_utc_ms, occurrence_delivery_correlation_id, occurrence_open_delivery_protocol, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_attempt_count, occurrence_superseded_by_revision, obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_occurrence_mob_failed(arg_occurrence_id, arg_attempt_count, arg_failure_class) ==
    /\ ~([machine |-> "occurrence", variant |-> "MobDeliveryFailed", payload |-> [attempt_count |-> arg_attempt_count, failure_class |-> arg_failure_class, occurrence_id |-> arg_occurrence_id], source_kind |-> "entry", source_route |-> "occurrence_mob_failed", source_machine |-> "external_entry", source_effect |-> "MobDeliveryFailed", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "occurrence", variant |-> "MobDeliveryFailed", payload |-> [attempt_count |-> arg_attempt_count, failure_class |-> arg_failure_class, occurrence_id |-> arg_occurrence_id], source_kind |-> "entry", source_route |-> "occurrence_mob_failed", source_machine |-> "external_entry", source_effect |-> "MobDeliveryFailed", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "occurrence", variant |-> "MobDeliveryFailed", payload |-> [attempt_count |-> arg_attempt_count, failure_class |-> arg_failure_class, occurrence_id |-> arg_occurrence_id], source_kind |-> "entry", source_route |-> "occurrence_mob_failed", source_machine |-> "external_entry", source_effect |-> "MobDeliveryFailed", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_lease_held, occurrence_lease_owner, occurrence_lease_expiry_utc_ms, occurrence_delivery_correlation_id, occurrence_open_delivery_protocol, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_attempt_count, occurrence_superseded_by_revision, obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

Inject_occurrence_lease_expired ==
    /\ ~([machine |-> "occurrence", variant |-> "LeaseExpired", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "occurrence_lease_expired", source_machine |-> "external_entry", source_effect |-> "LeaseExpired", effect_id |-> 0] \in SeqElements(pending_inputs))
    /\ pending_inputs' = Append(pending_inputs, [machine |-> "occurrence", variant |-> "LeaseExpired", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "occurrence_lease_expired", source_machine |-> "external_entry", source_effect |-> "LeaseExpired", effect_id |-> 0])
    /\ observed_inputs' = observed_inputs \cup {[machine |-> "occurrence", variant |-> "LeaseExpired", payload |-> [tag |-> "unit"], source_kind |-> "entry", source_route |-> "occurrence_lease_expired", source_machine |-> "external_entry", source_effect |-> "LeaseExpired", effect_id |-> 0]}
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_lease_held, occurrence_lease_owner, occurrence_lease_expiry_utc_ms, occurrence_delivery_correlation_id, occurrence_open_delivery_protocol, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_attempt_count, occurrence_superseded_by_revision, obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

DeliverQueuedRoute ==
    /\ Len(pending_routes) > 0
    /\ LET route == Head(pending_routes) IN
       /\ pending_routes' = Tail(pending_routes)
       /\ delivered_routes' = delivered_routes \cup {route}
       /\ model_step_count' = model_step_count + 1
       /\ pending_inputs' = AppendIfMissing(pending_inputs, [machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id]}
       /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_lease_held, occurrence_lease_owner, occurrence_lease_expiry_utc_ms, occurrence_delivery_correlation_id, occurrence_open_delivery_protocol, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_attempt_count, occurrence_superseded_by_revision, obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

QuiescentStutter ==
    /\ Len(pending_routes) = 0
    /\ Len(pending_inputs) = 0
    /\ UNCHANGED vars

WitnessInjectNext_mob_delivery_completion_path ==
    /\ witness_current_script_input # None
    /\ ~(witness_current_script_input \in SeqElements(pending_inputs))
    /\ Len(pending_inputs) = 0
    /\ Len(pending_routes) = 0
    /\ Len(witness_remaining_script_inputs) > 0
    /\ pending_inputs' = Append(pending_inputs, Head(witness_remaining_script_inputs))
    /\ observed_inputs' = observed_inputs \cup {Head(witness_remaining_script_inputs)}
    /\ witness_current_script_input' = Head(witness_remaining_script_inputs)
    /\ witness_remaining_script_inputs' = Tail(witness_remaining_script_inputs)
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_lease_held, occurrence_lease_owner, occurrence_lease_expiry_utc_ms, occurrence_delivery_correlation_id, occurrence_open_delivery_protocol, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_attempt_count, occurrence_superseded_by_revision, obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

WitnessInjectNext_mob_delivery_failure_path ==
    /\ witness_current_script_input # None
    /\ ~(witness_current_script_input \in SeqElements(pending_inputs))
    /\ Len(pending_inputs) = 0
    /\ Len(pending_routes) = 0
    /\ Len(witness_remaining_script_inputs) > 0
    /\ pending_inputs' = Append(pending_inputs, Head(witness_remaining_script_inputs))
    /\ observed_inputs' = observed_inputs \cup {Head(witness_remaining_script_inputs)}
    /\ witness_current_script_input' = Head(witness_remaining_script_inputs)
    /\ witness_remaining_script_inputs' = Tail(witness_remaining_script_inputs)
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_lease_held, occurrence_lease_owner, occurrence_lease_expiry_utc_ms, occurrence_delivery_correlation_id, occurrence_open_delivery_protocol, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_attempt_count, occurrence_superseded_by_revision, obligation_occurrence_runtime_delivery, obligation_occurrence_mob_delivery, pending_routes, delivered_routes, emitted_effects, observed_transitions >>

CoreNext ==
    \/ \E arg_claim_time_utc_ms \in 0..2 : \E arg_owner_id \in {"alpha", "beta"} : \E arg_lease_expiry_utc_ms \in 0..2 : occurrence_ClaimPending(arg_claim_time_utc_ms, arg_owner_id, arg_lease_expiry_utc_ms)
    \/ \E arg_delivery_correlation_id \in {"alpha", "beta"} : occurrence_StartRuntimeDispatchFromClaimed(arg_delivery_correlation_id)
    \/ \E arg_delivery_correlation_id \in {"alpha", "beta"} : occurrence_StartMobDispatchFromClaimed(arg_delivery_correlation_id)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : occurrence_RuntimeAcceptedFromDispatching(arg_occurrence_id, arg_attempt_count)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : occurrence_MobAcceptedFromDispatching(arg_occurrence_id, arg_attempt_count)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : occurrence_RuntimeCompletedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : occurrence_MobCompletedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : occurrence_RuntimeSkippedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : occurrence_MobSkippedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : occurrence_RuntimeMisfiredFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : occurrence_MobMisfiredFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : occurrence_RuntimeDeliveryFailedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : occurrence_MobDeliveryFailedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)
    \/ \E arg_superseding_revision \in 0..2 : occurrence_SupersedePending(arg_superseding_revision)
    \/ occurrence_LeaseExpiredFromClaimed
    \/ occurrence_LeaseExpiredFromDispatching
    \/ occurrence_LeaseExpiredFromAwaitingCompletion
    \/ QuiescentStutter

InjectNext ==
    \/ \E arg_claim_time_utc_ms \in 0..2 : \E arg_owner_id \in {"alpha", "beta"} : \E arg_lease_expiry_utc_ms \in 0..2 : Inject_occurrence_claim(arg_claim_time_utc_ms, arg_owner_id, arg_lease_expiry_utc_ms)
    \/ \E arg_delivery_correlation_id \in {"alpha", "beta"} : Inject_occurrence_start_mob_dispatch(arg_delivery_correlation_id)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : Inject_occurrence_mob_accepted(arg_occurrence_id, arg_attempt_count)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : Inject_occurrence_mob_completed(arg_occurrence_id, arg_attempt_count)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : Inject_occurrence_mob_skipped(arg_occurrence_id, arg_attempt_count, arg_failure_class)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : Inject_occurrence_mob_misfired(arg_occurrence_id, arg_attempt_count, arg_failure_class)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : Inject_occurrence_mob_failed(arg_occurrence_id, arg_attempt_count, arg_failure_class)
    \/ Inject_occurrence_lease_expired

Next ==
    \/ CoreNext
    \/ InjectNext

WitnessNext_mob_delivery_completion_path ==
    \/ CoreNext
    \/ WitnessInjectNext_mob_delivery_completion_path

WitnessNext_mob_delivery_failure_path ==
    \/ CoreNext
    \/ WitnessInjectNext_mob_delivery_failure_path


mob_delivery_protocol_covered == TRUE

NoOpenObligationsOnTerminal_occurrence_runtime_delivery == (occurrence_phase = "Completed" \/ occurrence_phase = "Skipped" \/ occurrence_phase = "Misfired" \/ occurrence_phase = "Superseded" \/ occurrence_phase = "DeliveryFailed") => obligation_occurrence_runtime_delivery = {}
NoFeedbackWithoutObligation_occurrence_runtime_delivery == \A input_packet \in observed_inputs : ((((input_packet.machine = "occurrence" /\ input_packet.variant = "RuntimeAccepted")) => ((\E record \in obligation_occurrence_runtime_delivery : (record.occurrence_id = input_packet.payload.occurrence_id /\ record.attempt_count = input_packet.payload.attempt_count)))) /\ (((input_packet.machine = "occurrence" /\ input_packet.variant = "RuntimeCompleted")) => ((\E record \in obligation_occurrence_runtime_delivery : (record.occurrence_id = input_packet.payload.occurrence_id /\ record.attempt_count = input_packet.payload.attempt_count)))) /\ (((input_packet.machine = "occurrence" /\ input_packet.variant = "RuntimeSkipped")) => ((\E record \in obligation_occurrence_runtime_delivery : (record.occurrence_id = input_packet.payload.occurrence_id /\ record.attempt_count = input_packet.payload.attempt_count)))) /\ (((input_packet.machine = "occurrence" /\ input_packet.variant = "RuntimeMisfired")) => ((\E record \in obligation_occurrence_runtime_delivery : (record.occurrence_id = input_packet.payload.occurrence_id /\ record.attempt_count = input_packet.payload.attempt_count)))) /\ (((input_packet.machine = "occurrence" /\ input_packet.variant = "RuntimeDeliveryFailed")) => ((\E record \in obligation_occurrence_runtime_delivery : (record.occurrence_id = input_packet.payload.occurrence_id /\ record.attempt_count = input_packet.payload.attempt_count)))))
NoOpenObligationsOnTerminal_occurrence_mob_delivery == (occurrence_phase = "Completed" \/ occurrence_phase = "Skipped" \/ occurrence_phase = "Misfired" \/ occurrence_phase = "Superseded" \/ occurrence_phase = "DeliveryFailed") => obligation_occurrence_mob_delivery = {}
NoFeedbackWithoutObligation_occurrence_mob_delivery == \A input_packet \in observed_inputs : ((((input_packet.machine = "occurrence" /\ input_packet.variant = "MobAccepted")) => ((\E record \in obligation_occurrence_mob_delivery : (record.occurrence_id = input_packet.payload.occurrence_id /\ record.attempt_count = input_packet.payload.attempt_count)))) /\ (((input_packet.machine = "occurrence" /\ input_packet.variant = "MobCompleted")) => ((\E record \in obligation_occurrence_mob_delivery : (record.occurrence_id = input_packet.payload.occurrence_id /\ record.attempt_count = input_packet.payload.attempt_count)))) /\ (((input_packet.machine = "occurrence" /\ input_packet.variant = "MobSkipped")) => ((\E record \in obligation_occurrence_mob_delivery : (record.occurrence_id = input_packet.payload.occurrence_id /\ record.attempt_count = input_packet.payload.attempt_count)))) /\ (((input_packet.machine = "occurrence" /\ input_packet.variant = "MobMisfired")) => ((\E record \in obligation_occurrence_mob_delivery : (record.occurrence_id = input_packet.payload.occurrence_id /\ record.attempt_count = input_packet.payload.attempt_count)))) /\ (((input_packet.machine = "occurrence" /\ input_packet.variant = "MobDeliveryFailed")) => ((\E record \in obligation_occurrence_mob_delivery : (record.occurrence_id = input_packet.payload.occurrence_id /\ record.attempt_count = input_packet.payload.attempt_count)))))

\* Liveness: eventual feedback or lease expiry under runtime owner fairness
OwnerFeedback_occurrence_runtime_delivery ==
    /\ obligation_occurrence_runtime_delivery /= {}
    /\ \E token \in obligation_occurrence_runtime_delivery :
        /\ ((/\ pending_inputs' = Append(pending_inputs, [machine |-> "occurrence", variant |-> "RuntimeAccepted", source_kind |-> "owner", source_machine |-> "occurrence", source_effect |-> "DispatchToRuntime", source_route |-> "none", effect_id |-> token, payload |-> [occurrence_id |-> token.occurrence_id, attempt_count |-> token.attempt_count]]) /\ obligation_occurrence_runtime_delivery' = obligation_occurrence_runtime_delivery \ {token}) \/ (/\ pending_inputs' = Append(pending_inputs, [machine |-> "occurrence", variant |-> "RuntimeCompleted", source_kind |-> "owner", source_machine |-> "occurrence", source_effect |-> "DispatchToRuntime", source_route |-> "none", effect_id |-> token, payload |-> [occurrence_id |-> token.occurrence_id, attempt_count |-> token.attempt_count]]) /\ obligation_occurrence_runtime_delivery' = obligation_occurrence_runtime_delivery \ {token}) \/ \E owner_ctx_runtime_skipped_failure_class \in OccurrenceFailureClassValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "occurrence", variant |-> "RuntimeSkipped", source_kind |-> "owner", source_machine |-> "occurrence", source_effect |-> "DispatchToRuntime", source_route |-> "none", effect_id |-> token, payload |-> [occurrence_id |-> token.occurrence_id, attempt_count |-> token.attempt_count, failure_class |-> owner_ctx_runtime_skipped_failure_class]]) /\ obligation_occurrence_runtime_delivery' = obligation_occurrence_runtime_delivery \ {token}) \/ \E owner_ctx_runtime_misfired_failure_class \in OccurrenceFailureClassValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "occurrence", variant |-> "RuntimeMisfired", source_kind |-> "owner", source_machine |-> "occurrence", source_effect |-> "DispatchToRuntime", source_route |-> "none", effect_id |-> token, payload |-> [occurrence_id |-> token.occurrence_id, attempt_count |-> token.attempt_count, failure_class |-> owner_ctx_runtime_misfired_failure_class]]) /\ obligation_occurrence_runtime_delivery' = obligation_occurrence_runtime_delivery \ {token}) \/ \E owner_ctx_runtime_delivery_failed_failure_class \in OccurrenceFailureClassValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "occurrence", variant |-> "RuntimeDeliveryFailed", source_kind |-> "owner", source_machine |-> "occurrence", source_effect |-> "DispatchToRuntime", source_route |-> "none", effect_id |-> token, payload |-> [occurrence_id |-> token.occurrence_id, attempt_count |-> token.attempt_count, failure_class |-> owner_ctx_runtime_delivery_failed_failure_class]]) /\ obligation_occurrence_runtime_delivery' = obligation_occurrence_runtime_delivery \ {token}))
    /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_lease_held, occurrence_lease_owner, occurrence_lease_expiry_utc_ms, occurrence_delivery_correlation_id, occurrence_open_delivery_protocol, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_attempt_count, occurrence_superseded_by_revision, obligation_occurrence_mob_delivery, model_step_count, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

\* Liveness: eventual feedback or lease expiry under mob owner fairness
OwnerFeedback_occurrence_mob_delivery ==
    /\ obligation_occurrence_mob_delivery /= {}
    /\ \E token \in obligation_occurrence_mob_delivery :
        /\ ((/\ pending_inputs' = Append(pending_inputs, [machine |-> "occurrence", variant |-> "MobAccepted", source_kind |-> "owner", source_machine |-> "occurrence", source_effect |-> "DispatchToMob", source_route |-> "none", effect_id |-> token, payload |-> [occurrence_id |-> token.occurrence_id, attempt_count |-> token.attempt_count]]) /\ obligation_occurrence_mob_delivery' = obligation_occurrence_mob_delivery \ {token}) \/ (/\ pending_inputs' = Append(pending_inputs, [machine |-> "occurrence", variant |-> "MobCompleted", source_kind |-> "owner", source_machine |-> "occurrence", source_effect |-> "DispatchToMob", source_route |-> "none", effect_id |-> token, payload |-> [occurrence_id |-> token.occurrence_id, attempt_count |-> token.attempt_count]]) /\ obligation_occurrence_mob_delivery' = obligation_occurrence_mob_delivery \ {token}) \/ \E owner_ctx_mob_skipped_failure_class \in OccurrenceFailureClassValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "occurrence", variant |-> "MobSkipped", source_kind |-> "owner", source_machine |-> "occurrence", source_effect |-> "DispatchToMob", source_route |-> "none", effect_id |-> token, payload |-> [occurrence_id |-> token.occurrence_id, attempt_count |-> token.attempt_count, failure_class |-> owner_ctx_mob_skipped_failure_class]]) /\ obligation_occurrence_mob_delivery' = obligation_occurrence_mob_delivery \ {token}) \/ \E owner_ctx_mob_misfired_failure_class \in OccurrenceFailureClassValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "occurrence", variant |-> "MobMisfired", source_kind |-> "owner", source_machine |-> "occurrence", source_effect |-> "DispatchToMob", source_route |-> "none", effect_id |-> token, payload |-> [occurrence_id |-> token.occurrence_id, attempt_count |-> token.attempt_count, failure_class |-> owner_ctx_mob_misfired_failure_class]]) /\ obligation_occurrence_mob_delivery' = obligation_occurrence_mob_delivery \ {token}) \/ \E owner_ctx_mob_delivery_failed_failure_class \in OccurrenceFailureClassValues : (/\ pending_inputs' = Append(pending_inputs, [machine |-> "occurrence", variant |-> "MobDeliveryFailed", source_kind |-> "owner", source_machine |-> "occurrence", source_effect |-> "DispatchToMob", source_route |-> "none", effect_id |-> token, payload |-> [occurrence_id |-> token.occurrence_id, attempt_count |-> token.attempt_count, failure_class |-> owner_ctx_mob_delivery_failed_failure_class]]) /\ obligation_occurrence_mob_delivery' = obligation_occurrence_mob_delivery \ {token}))
    /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_lease_held, occurrence_lease_owner, occurrence_lease_expiry_utc_ms, occurrence_delivery_correlation_id, occurrence_open_delivery_protocol, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_attempt_count, occurrence_superseded_by_revision, obligation_occurrence_runtime_delivery, model_step_count, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

CoverageInstrumentation == TRUE

CiStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 1 /\ Cardinality(observed_inputs) <= 4 /\ Len(pending_routes) <= 1 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 1 /\ Cardinality(observed_transitions) <= 6
DeepStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 2 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 6
WitnessStateConstraint_mob_delivery_completion_path == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 1 /\ Cardinality(observed_inputs) <= 4 /\ Len(pending_routes) <= 1 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 4 /\ Cardinality(observed_transitions) <= 8
WitnessStateConstraint_mob_delivery_failure_path == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 1 /\ Cardinality(observed_inputs) <= 4 /\ Len(pending_routes) <= 1 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 3 /\ Cardinality(observed_transitions) <= 6

Spec == Init /\ [][Next]_vars
WitnessSpec_mob_delivery_completion_path == WitnessInit_mob_delivery_completion_path /\ [] [WitnessNext_mob_delivery_completion_path]_vars /\ WF_vars(\E arg_claim_time_utc_ms \in 0..2 : \E arg_owner_id \in {"alpha", "beta"} : \E arg_lease_expiry_utc_ms \in 0..2 : occurrence_ClaimPending(arg_claim_time_utc_ms, arg_owner_id, arg_lease_expiry_utc_ms)) /\ WF_vars(\E arg_delivery_correlation_id \in {"alpha", "beta"} : occurrence_StartRuntimeDispatchFromClaimed(arg_delivery_correlation_id)) /\ WF_vars(\E arg_delivery_correlation_id \in {"alpha", "beta"} : occurrence_StartMobDispatchFromClaimed(arg_delivery_correlation_id)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : occurrence_RuntimeAcceptedFromDispatching(arg_occurrence_id, arg_attempt_count)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : occurrence_MobAcceptedFromDispatching(arg_occurrence_id, arg_attempt_count)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : occurrence_RuntimeCompletedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : occurrence_MobCompletedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : occurrence_RuntimeSkippedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : occurrence_MobSkippedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : occurrence_RuntimeMisfiredFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : occurrence_MobMisfiredFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : occurrence_RuntimeDeliveryFailedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : occurrence_MobDeliveryFailedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)) /\ WF_vars(\E arg_superseding_revision \in 0..2 : occurrence_SupersedePending(arg_superseding_revision)) /\ WF_vars(occurrence_LeaseExpiredFromClaimed) /\ WF_vars(occurrence_LeaseExpiredFromDispatching) /\ WF_vars(occurrence_LeaseExpiredFromAwaitingCompletion) /\ WF_vars(WitnessInjectNext_mob_delivery_completion_path)
WitnessSpec_mob_delivery_failure_path == WitnessInit_mob_delivery_failure_path /\ [] [WitnessNext_mob_delivery_failure_path]_vars /\ WF_vars(\E arg_claim_time_utc_ms \in 0..2 : \E arg_owner_id \in {"alpha", "beta"} : \E arg_lease_expiry_utc_ms \in 0..2 : occurrence_ClaimPending(arg_claim_time_utc_ms, arg_owner_id, arg_lease_expiry_utc_ms)) /\ WF_vars(\E arg_delivery_correlation_id \in {"alpha", "beta"} : occurrence_StartRuntimeDispatchFromClaimed(arg_delivery_correlation_id)) /\ WF_vars(\E arg_delivery_correlation_id \in {"alpha", "beta"} : occurrence_StartMobDispatchFromClaimed(arg_delivery_correlation_id)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : occurrence_RuntimeAcceptedFromDispatching(arg_occurrence_id, arg_attempt_count)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : occurrence_MobAcceptedFromDispatching(arg_occurrence_id, arg_attempt_count)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : occurrence_RuntimeCompletedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : occurrence_MobCompletedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : occurrence_RuntimeSkippedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : occurrence_MobSkippedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : occurrence_RuntimeMisfiredFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : occurrence_MobMisfiredFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : occurrence_RuntimeDeliveryFailedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : occurrence_MobDeliveryFailedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)) /\ WF_vars(\E arg_superseding_revision \in 0..2 : occurrence_SupersedePending(arg_superseding_revision)) /\ WF_vars(occurrence_LeaseExpiredFromClaimed) /\ WF_vars(occurrence_LeaseExpiredFromDispatching) /\ WF_vars(occurrence_LeaseExpiredFromAwaitingCompletion) /\ WF_vars(WitnessInjectNext_mob_delivery_failure_path)

WitnessStateObserved_mob_delivery_completion_path_1 == <> (occurrence_phase = "Completed")
WitnessTransitionObserved_mob_delivery_completion_path_occurrence_ClaimPending == <> (\E packet \in observed_transitions : /\ packet.machine = "occurrence" /\ packet.transition = "ClaimPending")
WitnessTransitionObserved_mob_delivery_completion_path_occurrence_StartMobDispatchFromClaimed == <> (\E packet \in observed_transitions : /\ packet.machine = "occurrence" /\ packet.transition = "StartMobDispatchFromClaimed")
WitnessTransitionObserved_mob_delivery_completion_path_occurrence_MobAcceptedFromDispatching == <> (\E packet \in observed_transitions : /\ packet.machine = "occurrence" /\ packet.transition = "MobAcceptedFromDispatching")
WitnessTransitionObserved_mob_delivery_completion_path_occurrence_MobCompletedFromDispatchingOrAwaiting == <> (\E packet \in observed_transitions : /\ packet.machine = "occurrence" /\ packet.transition = "MobCompletedFromDispatchingOrAwaiting")
WitnessTransitionOrder_mob_delivery_completion_path_1 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "occurrence" /\ earlier.transition = "ClaimPending" /\ later.machine = "occurrence" /\ later.transition = "StartMobDispatchFromClaimed" /\ earlier.step < later.step)
WitnessTransitionOrder_mob_delivery_completion_path_2 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "occurrence" /\ earlier.transition = "StartMobDispatchFromClaimed" /\ later.machine = "occurrence" /\ later.transition = "MobAcceptedFromDispatching" /\ earlier.step < later.step)
WitnessTransitionOrder_mob_delivery_completion_path_3 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "occurrence" /\ earlier.transition = "MobAcceptedFromDispatching" /\ later.machine = "occurrence" /\ later.transition = "MobCompletedFromDispatchingOrAwaiting" /\ earlier.step < later.step)
WitnessStateObserved_mob_delivery_failure_path_1 == <> (occurrence_phase = "DeliveryFailed")
WitnessTransitionObserved_mob_delivery_failure_path_occurrence_ClaimPending == <> (\E packet \in observed_transitions : /\ packet.machine = "occurrence" /\ packet.transition = "ClaimPending")
WitnessTransitionObserved_mob_delivery_failure_path_occurrence_StartMobDispatchFromClaimed == <> (\E packet \in observed_transitions : /\ packet.machine = "occurrence" /\ packet.transition = "StartMobDispatchFromClaimed")
WitnessTransitionObserved_mob_delivery_failure_path_occurrence_MobDeliveryFailedFromDispatchingOrAwaiting == <> (\E packet \in observed_transitions : /\ packet.machine = "occurrence" /\ packet.transition = "MobDeliveryFailedFromDispatchingOrAwaiting")
WitnessTransitionOrder_mob_delivery_failure_path_1 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "occurrence" /\ earlier.transition = "ClaimPending" /\ later.machine = "occurrence" /\ later.transition = "StartMobDispatchFromClaimed" /\ earlier.step < later.step)
WitnessTransitionOrder_mob_delivery_failure_path_2 == <> (\E earlier \in observed_transitions, later \in observed_transitions : /\ earlier.machine = "occurrence" /\ earlier.transition = "StartMobDispatchFromClaimed" /\ later.machine = "occurrence" /\ later.transition = "MobDeliveryFailedFromDispatchingOrAwaiting" /\ earlier.step < later.step)

THEOREM Spec => []mob_delivery_protocol_covered
THEOREM Spec => []occurrence_terminal_has_no_open_delivery_protocol
THEOREM Spec => []occurrence_live_claim_requires_lease_holder
THEOREM Spec => []occurrence_awaiting_completion_requires_protocol
THEOREM Spec => []occurrence_superseded_records_revision
THEOREM Spec => []occurrence_delivery_failed_records_failure_class
THEOREM Spec => []NoOpenObligationsOnTerminal_occurrence_runtime_delivery
THEOREM Spec => []NoFeedbackWithoutObligation_occurrence_runtime_delivery
THEOREM Spec => []NoOpenObligationsOnTerminal_occurrence_mob_delivery
THEOREM Spec => []NoFeedbackWithoutObligation_occurrence_mob_delivery

=============================================================================
