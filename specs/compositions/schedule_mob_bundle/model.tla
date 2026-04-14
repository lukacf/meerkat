---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for schedule_mob_bundle.

CONSTANTS DeliveryReceiptStageValues, NatValues, OccurrenceFailureClassValues, OccurrenceIdValues, ScheduleIdValues, StringValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionDeliveryReceiptStageValues == {None} \cup {Some(x) : x \in DeliveryReceiptStageValues}
OptionOccurrenceFailureClassValues == {None} \cup {Some(x) : x \in OccurrenceFailureClassValues}
OptionStringValues == {None} \cup {Some(x) : x \in StringValues}
OptionU64Values == {None} \cup {Some(x) : x \in NatValues}

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

RouteTargetKind(route_name) ==
    "Unknown"

RouteDeliveryKind(route_name) ==
    "Unknown"

RouteTargetActor(route_name) == ActorOfMachine(RouteTargetMachine(route_name))

VARIABLES occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

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
    /\ occurrence_claimed_by = None
    /\ occurrence_lease_expires_at_utc_ms = None
    /\ occurrence_claimed_at_utc_ms = None
    /\ occurrence_claim_token = None
    /\ occurrence_delivery_correlation_id = None
    /\ occurrence_last_receipt_stage = None
    /\ occurrence_failure_class = None
    /\ occurrence_failure_detail = None
    /\ occurrence_dispatched_at_utc_ms = None
    /\ occurrence_completed_at_utc_ms = None
    /\ occurrence_attempt_count = 0
    /\ occurrence_superseded_by_revision = None
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

WitnessInit_mob_delivery_feedback ==
    /\ BaseInit
    /\ pending_inputs = <<>>
    /\ observed_inputs = {}
    /\ witness_current_script_input = None
    /\ witness_remaining_script_inputs = <<>>

WitnessInit_materialization_failure_classification ==
    /\ BaseInit
    /\ pending_inputs = <<>>
    /\ observed_inputs = {}
    /\ witness_current_script_input = None
    /\ witness_remaining_script_inputs = <<>>

occurrence__is_live_claim_phase(arg_phase) == ((arg_phase = "Claimed") \/ (arg_phase = "Dispatching") \/ (arg_phase = "AwaitingCompletion"))

occurrence_ClaimPending(arg_owner_id, arg_at_utc_ms, arg_lease_expires_at_utc_ms, arg_claim_token) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "Claim"
       /\ packet.payload.owner_id = arg_owner_id
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ packet.payload.lease_expires_at_utc_ms = arg_lease_expires_at_utc_ms
       /\ packet.payload.claim_token = arg_claim_token
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending"
       /\ occurrence_phase' = "Claimed"
       /\ occurrence_claimed_by' = Some(packet.payload.owner_id)
       /\ occurrence_lease_expires_at_utc_ms' = Some(packet.payload.lease_expires_at_utc_ms)
       /\ occurrence_claimed_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ occurrence_claim_token' = Some(packet.payload.claim_token)
       /\ occurrence_delivery_correlation_id' = None
       /\ occurrence_last_receipt_stage' = None
       /\ occurrence_failure_class' = None
       /\ occurrence_failure_detail' = None
       /\ occurrence_dispatched_at_utc_ms' = None
       /\ occurrence_completed_at_utc_ms' = None
       /\ occurrence_attempt_count' = (occurrence_attempt_count) + 1
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "Claimed", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ClaimPending"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClaimPending", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Claimed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_DispatchStartedFromClaimed(arg_correlation_id, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "DispatchStarted"
       /\ packet.payload.correlation_id = arg_correlation_id
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed"
       /\ occurrence_phase' = "Dispatching"
       /\ occurrence_delivery_correlation_id' = packet.payload.correlation_id
       /\ occurrence_dispatched_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_failure_detail, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DispatchStarted", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "DispatchStartedFromClaimed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "DispatchStartedFromClaimed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Dispatching"]}
       /\ model_step_count' = model_step_count + 1


occurrence_AwaitCompletionFromDispatching(arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "AwaitCompletion"
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching"
       /\ occurrence_phase' = "AwaitingCompletion"
       /\ occurrence_dispatched_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_failure_detail, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "AwaitingCompletion", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AwaitCompletionFromDispatching"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "AwaitCompletionFromDispatching", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "AwaitingCompletion"]}
       /\ model_step_count' = model_step_count + 1


occurrence_CompleteFromDispatchingOrAwaiting(arg_receipt_stage, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "Complete"
       /\ packet.payload.receipt_stage = arg_receipt_stage
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ occurrence_phase' = "Completed"
       /\ occurrence_last_receipt_stage' = Some(packet.payload.receipt_stage)
       /\ occurrence_completed_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "Completed", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteFromDispatchingOrAwaiting"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "CompleteFromDispatchingOrAwaiting", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_SkipFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "Skip"
       /\ packet.payload.detail = arg_detail
       /\ packet.payload.failure_class = arg_failure_class
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending" \/ occurrence_phase = "Claimed" \/ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ occurrence_phase' = "Skipped"
       /\ occurrence_failure_class' = packet.payload.failure_class
       /\ occurrence_failure_detail' = packet.payload.detail
       /\ occurrence_completed_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt_stage, occurrence_dispatched_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "Skipped", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "SkipFromPendingOrLive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "SkipFromPendingOrLive", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Skipped"]}
       /\ model_step_count' = model_step_count + 1


occurrence_MisfireFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "Misfire"
       /\ packet.payload.detail = arg_detail
       /\ packet.payload.failure_class = arg_failure_class
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending" \/ occurrence_phase = "Claimed" \/ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ occurrence_phase' = "Misfired"
       /\ occurrence_failure_class' = packet.payload.failure_class
       /\ occurrence_failure_detail' = packet.payload.detail
       /\ occurrence_completed_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt_stage, occurrence_dispatched_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "Misfired", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "MisfireFromPendingOrLive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "MisfireFromPendingOrLive", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Misfired"]}
       /\ model_step_count' = model_step_count + 1


occurrence_SupersedePendingOrLive(arg_superseded_by_revision, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "Supersede"
       /\ packet.payload.superseded_by_revision = arg_superseded_by_revision
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending" \/ occurrence_phase = "Claimed" \/ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ occurrence_phase' = "Superseded"
       /\ occurrence_completed_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ occurrence_superseded_by_revision' = Some(packet.payload.superseded_by_revision)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_attempt_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "Superseded", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "SupersedePendingOrLive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "SupersedePendingOrLive", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Superseded"]}
       /\ model_step_count' = model_step_count + 1


occurrence_DeliveryFailedFromClaimedOrLive(arg_receipt_stage, arg_failure_class, arg_detail, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "DeliveryFailed"
       /\ packet.payload.receipt_stage = arg_receipt_stage
       /\ packet.payload.failure_class = arg_failure_class
       /\ packet.payload.detail = arg_detail
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed" \/ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ occurrence_phase' = "DeliveryFailed"
       /\ occurrence_last_receipt_stage' = packet.payload.receipt_stage
       /\ occurrence_failure_class' = Some(packet.payload.failure_class)
       /\ occurrence_failure_detail' = packet.payload.detail
       /\ occurrence_completed_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_dispatched_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DeliveryFailed", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "DeliveryFailedFromClaimedOrLive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "DeliveryFailedFromClaimedOrLive", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "DeliveryFailed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_LeaseExpiredFromClaimed(arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "LeaseExpired"
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed"
       /\ occurrence_phase' = "Pending"
       /\ occurrence_claimed_by' = None
       /\ occurrence_lease_expires_at_utc_ms' = None
       /\ occurrence_claimed_at_utc_ms' = None
       /\ occurrence_claim_token' = None
       /\ occurrence_delivery_correlation_id' = None
       /\ occurrence_dispatched_at_utc_ms' = None
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_failure_detail, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "LeaseExpired", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "LeaseExpiredFromClaimed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "LeaseExpiredFromClaimed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_LeaseExpiredFromDispatching(arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "LeaseExpired"
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching"
       /\ occurrence_phase' = "Pending"
       /\ occurrence_claimed_by' = None
       /\ occurrence_lease_expires_at_utc_ms' = None
       /\ occurrence_claimed_at_utc_ms' = None
       /\ occurrence_claim_token' = None
       /\ occurrence_delivery_correlation_id' = None
       /\ occurrence_dispatched_at_utc_ms' = None
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_failure_detail, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "LeaseExpired", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "LeaseExpiredFromDispatching"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "LeaseExpiredFromDispatching", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_LeaseExpiredFromAwaitingCompletion(arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "LeaseExpired"
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "AwaitingCompletion"
       /\ occurrence_phase' = "Pending"
       /\ occurrence_claimed_by' = None
       /\ occurrence_lease_expires_at_utc_ms' = None
       /\ occurrence_claimed_at_utc_ms' = None
       /\ occurrence_claim_token' = None
       /\ occurrence_delivery_correlation_id' = None
       /\ occurrence_dispatched_at_utc_ms' = None
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_failure_detail, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "LeaseExpired", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "LeaseExpiredFromAwaitingCompletion"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "LeaseExpiredFromAwaitingCompletion", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_live_claim_requires_owner == (~(occurrence__is_live_claim_phase(occurrence_phase)) \/ (occurrence_claimed_by # None))
occurrence_superseded_records_revision == ((occurrence_phase # "Superseded") \/ (occurrence_superseded_by_revision # None))
occurrence_delivery_failed_records_failure_class == ((occurrence_phase # "DeliveryFailed") \/ (occurrence_failure_class # None))

DeliverQueuedRoute ==
    /\ Len(pending_routes) > 0
    /\ LET route == Head(pending_routes) IN
       /\ pending_routes' = Tail(pending_routes)
       /\ delivered_routes' = delivered_routes \cup {route}
       /\ model_step_count' = model_step_count + 1
       /\ pending_inputs' = AppendIfMissing(pending_inputs, [machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id]}
       /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt_stage, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

QuiescentStutter ==
    /\ Len(pending_routes) = 0
    /\ Len(pending_inputs) = 0
    /\ UNCHANGED vars

WitnessInjectNext_mob_delivery_feedback ==
    FALSE

WitnessInjectNext_materialization_failure_classification ==
    FALSE

CoreNext ==
    \/ \E arg_owner_id \in StringValues : \E arg_at_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in 0..2 : \E arg_claim_token \in StringValues : occurrence_ClaimPending(arg_owner_id, arg_at_utc_ms, arg_lease_expires_at_utc_ms, arg_claim_token)
    \/ \E arg_correlation_id \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DispatchStartedFromClaimed(arg_correlation_id, arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : occurrence_AwaitCompletionFromDispatching(arg_at_utc_ms)
    \/ \E arg_receipt_stage \in DeliveryReceiptStageValues : \E arg_at_utc_ms \in 0..2 : occurrence_CompleteFromDispatchingOrAwaiting(arg_receipt_stage, arg_at_utc_ms)
    \/ \E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_SkipFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms)
    \/ \E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_MisfireFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms)
    \/ \E arg_superseded_by_revision \in 0..2 : \E arg_at_utc_ms \in 0..2 : occurrence_SupersedePendingOrLive(arg_superseded_by_revision, arg_at_utc_ms)
    \/ \E arg_receipt_stage \in OptionDeliveryReceiptStageValues : \E arg_failure_class \in OccurrenceFailureClassValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DeliveryFailedFromClaimedOrLive(arg_receipt_stage, arg_failure_class, arg_detail, arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromClaimed(arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromDispatching(arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromAwaitingCompletion(arg_at_utc_ms)
    \/ QuiescentStutter

InjectNext ==
    FALSE

Next ==
    \/ CoreNext

WitnessNext_mob_delivery_feedback ==
    \/ CoreNext
    \/ WitnessInjectNext_mob_delivery_feedback

WitnessNext_materialization_failure_classification ==
    \/ CoreNext
    \/ WitnessInjectNext_materialization_failure_classification


CoverageInstrumentation == TRUE

CiStateConstraint == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8
DeepStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 2 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 6
WitnessStateConstraint_mob_delivery_feedback == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8
WitnessStateConstraint_materialization_failure_classification == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8

Spec == Init /\ [][Next]_vars
WitnessSpec_mob_delivery_feedback == WitnessInit_mob_delivery_feedback /\ [] [WitnessNext_mob_delivery_feedback]_vars /\ WF_vars(\E arg_owner_id \in StringValues : \E arg_at_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in 0..2 : \E arg_claim_token \in StringValues : occurrence_ClaimPending(arg_owner_id, arg_at_utc_ms, arg_lease_expires_at_utc_ms, arg_claim_token)) /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DispatchStartedFromClaimed(arg_correlation_id, arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_AwaitCompletionFromDispatching(arg_at_utc_ms)) /\ WF_vars(\E arg_receipt_stage \in DeliveryReceiptStageValues : \E arg_at_utc_ms \in 0..2 : occurrence_CompleteFromDispatchingOrAwaiting(arg_receipt_stage, arg_at_utc_ms)) /\ WF_vars(\E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_SkipFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms)) /\ WF_vars(\E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_MisfireFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms)) /\ WF_vars(\E arg_superseded_by_revision \in 0..2 : \E arg_at_utc_ms \in 0..2 : occurrence_SupersedePendingOrLive(arg_superseded_by_revision, arg_at_utc_ms)) /\ WF_vars(\E arg_receipt_stage \in OptionDeliveryReceiptStageValues : \E arg_failure_class \in OccurrenceFailureClassValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DeliveryFailedFromClaimedOrLive(arg_receipt_stage, arg_failure_class, arg_detail, arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromClaimed(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromDispatching(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromAwaitingCompletion(arg_at_utc_ms))
WitnessSpec_materialization_failure_classification == WitnessInit_materialization_failure_classification /\ [] [WitnessNext_materialization_failure_classification]_vars /\ WF_vars(\E arg_owner_id \in StringValues : \E arg_at_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in 0..2 : \E arg_claim_token \in StringValues : occurrence_ClaimPending(arg_owner_id, arg_at_utc_ms, arg_lease_expires_at_utc_ms, arg_claim_token)) /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DispatchStartedFromClaimed(arg_correlation_id, arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_AwaitCompletionFromDispatching(arg_at_utc_ms)) /\ WF_vars(\E arg_receipt_stage \in DeliveryReceiptStageValues : \E arg_at_utc_ms \in 0..2 : occurrence_CompleteFromDispatchingOrAwaiting(arg_receipt_stage, arg_at_utc_ms)) /\ WF_vars(\E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_SkipFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms)) /\ WF_vars(\E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_MisfireFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms)) /\ WF_vars(\E arg_superseded_by_revision \in 0..2 : \E arg_at_utc_ms \in 0..2 : occurrence_SupersedePendingOrLive(arg_superseded_by_revision, arg_at_utc_ms)) /\ WF_vars(\E arg_receipt_stage \in OptionDeliveryReceiptStageValues : \E arg_failure_class \in OccurrenceFailureClassValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DeliveryFailedFromClaimedOrLive(arg_receipt_stage, arg_failure_class, arg_detail, arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromClaimed(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromDispatching(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromAwaitingCompletion(arg_at_utc_ms))


THEOREM Spec => []occurrence_live_claim_requires_owner
THEOREM Spec => []occurrence_superseded_records_revision
THEOREM Spec => []occurrence_delivery_failed_records_failure_class

=============================================================================
